// SPDX-License-Identifier: AGPL-3.0-or-later
//! App-password sessions: `createSession`, `refreshSession`, `getSession`, `deleteSession`, and
//! bearer checks on authenticated endpoints.
//!
//! Tokens are JWT-shaped (`header.payload.signature`, base64url) so clients can read `exp`; the
//! signature is a fake (a hash), and the server only accepts tokens it issued itself.
//! `Authorization: Bearer <token>` and `Authorization: DPoP <token>` are both accepted (DPoP
//! proofs are not checked). `userpds://` requests are authenticated by the host and need no
//! token.

use lessdugin_core::host::HttpResponse;
use serde_json::{Map, Value, json};

use super::server::Incoming;
use super::state::{AccountStatus, NetState, Token};
use crate::resp;

fn b64(bytes: &[u8]) -> String {
    data_encoding::BASE64URL_NOPAD.encode(bytes)
}

fn auth_missing() -> HttpResponse {
    resp::xrpc_error(401, "AuthMissing", "Authentication Required")
}

fn invalid_token(message: &str) -> HttpResponse {
    resp::xrpc_error(400, "InvalidToken", message)
}

fn expired_token(message: &str) -> HttpResponse {
    resp::xrpc_error(400, "ExpiredToken", message)
}

impl NetState {
    /// Issues an access/refresh token pair for `did` at `base`.
    pub fn issue_session(
        &mut self,
        base: &str,
        did: &str,
        app_password: bool,
        now: i64,
    ) -> (String, String) {
        let (access_ttl, refresh_ttl, aud) = {
            let p = &self.pds[base];
            (
                p.cfg.access_ttl_ms,
                p.cfg.refresh_ttl_ms,
                p.server_did.clone(),
            )
        };
        let mut mint = |refresh: bool, ttl: i64| {
            let jti = self.token("jti");
            let (typ, scope) = match (refresh, app_password) {
                (true, _) => ("refresh+jwt", "com.atproto.refresh"),
                (false, true) => ("at+jwt", "com.atproto.appPass"),
                (false, false) => ("at+jwt", "com.atproto.access"),
            };
            let exp_ms = now.saturating_add(ttl);
            let h = b64(json!({ "typ": typ, "alg": "ES256K" })
                .to_string()
                .as_bytes());
            let p = b64(json!({
                "scope": scope,
                "sub": did,
                "aud": aud,
                "iat": now.div_euclid(1000),
                "exp": exp_ms.div_euclid(1000),
                "jti": jti,
            })
            .to_string()
            .as_bytes());
            let sig = b64(&lessdugin_atp::cid::sha256(format!("{h}.{p}").as_bytes()));
            let tok = format!("{h}.{p}.{sig}");
            (
                tok,
                Token {
                    did: did.to_owned(),
                    refresh,
                    exp_ms,
                    revoked: false,
                    app_password,
                },
            )
        };
        let access = mint(false, access_ttl);
        let refresh = mint(true, refresh_ttl);
        let pds = self.pds.get_mut(base).expect("known PDS");
        pds.tokens.insert(access.0.clone(), access.1);
        pds.tokens.insert(refresh.0.clone(), refresh.1);
        (access.0, refresh.0)
    }

    /// The bearer token of a request, if any.
    fn bearer<'a>(inc: &'a Incoming<'_>) -> Result<Option<&'a str>, HttpResponse> {
        let Some(h) = inc
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
        else {
            return Ok(None);
        };
        let (scheme, tok) = h.split_once(' ').unwrap_or((h, ""));
        if !(scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("dpop")) {
            return Err(invalid_token("Unexpected authorization type"));
        }
        Ok(Some(tok.trim()))
    }

    /// Authenticates a request at `base`; returns the account DID. `refresh` asks for a refresh
    /// token (`refreshSession`, `deleteSession`) instead of an access token.
    pub(crate) fn authenticate(
        &mut self,
        base: &str,
        route_userpds: Option<&str>,
        inc: &Incoming<'_>,
        now: i64,
        refresh: bool,
    ) -> Result<String, HttpResponse> {
        if let Some(did) = route_userpds {
            return match self.accounts.get(did) {
                Some(a) if a.known_at(base) => Ok(did.to_owned()),
                _ => Err(auth_missing()),
            };
        }
        let tok = Self::bearer(inc)?.ok_or_else(auth_missing)?;
        let Some(t) = self.pds.get(base).and_then(|p| p.tokens.get(tok)) else {
            return Err(invalid_token("Token could not be verified"));
        };
        if t.revoked {
            return Err(expired_token("Token has been revoked"));
        }
        if t.exp_ms <= now {
            return Err(expired_token("Token has expired"));
        }
        if t.refresh != refresh {
            return Err(invalid_token("Invalid token type"));
        }
        let did = t.did.clone();
        match self.accounts.get(&did) {
            Some(a) if a.known_at(base) && a.status == AccountStatus::Takendown => Err(
                resp::xrpc_error(401, "AccountTakedown", "Account has been taken down"),
            ),
            Some(a) if a.known_at(base) => Ok(did),
            _ => Err(invalid_token("Token could not be verified")),
        }
    }

    fn session_json(&self, did: &str, tokens: Option<(String, String)>) -> Value {
        let a = &self.accounts[did];
        let mut out = Map::new();
        if let Some((access, refresh)) = tokens {
            out.insert("accessJwt".to_owned(), Value::String(access));
            out.insert("refreshJwt".to_owned(), Value::String(refresh));
        }
        out.insert("handle".to_owned(), Value::String(a.handle.clone()));
        out.insert("did".to_owned(), Value::String(did.to_owned()));
        out.insert("didDoc".to_owned(), a.doc());
        out.insert("email".to_owned(), Value::String(a.email.clone()));
        out.insert("emailConfirmed".to_owned(), Value::Bool(true));
        out.insert("emailAuthFactor".to_owned(), Value::Bool(false));
        out.insert(
            "active".to_owned(),
            Value::Bool(a.status == AccountStatus::Active),
        );
        if let Some(s) = a.status.as_status() {
            out.insert("status".to_owned(), Value::String(s.to_owned()));
        }
        Value::Object(out)
    }

    pub(crate) fn create_session_impl(
        &mut self,
        base: &str,
        body: Option<&Value>,
        now: i64,
    ) -> Result<(String, Value), HttpResponse> {
        let Some(b) = body.and_then(Value::as_object) else {
            return Err(resp::xrpc_error(
                400,
                "InvalidRequest",
                "Request body must be a JSON object",
            ));
        };
        let field = |k: &str| b.get(k).and_then(Value::as_str);
        let (Some(ident), Some(password)) = (field("identifier"), field("password")) else {
            return Err(resp::xrpc_error(
                400,
                "InvalidRequest",
                "Input must have the properties \"identifier\" and \"password\"",
            ));
        };
        let allow_takendown = b
            .get("allowTakendown")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ident = ident.to_ascii_lowercase();
        let did = self
            .accounts
            .values()
            .find(|a| {
                a.known_at(base)
                    && (a.did.as_str() == ident || a.handle == ident || a.email == ident)
            })
            .map(|a| a.did.to_string());
        let unauthorized = || {
            resp::xrpc_error(
                401,
                "AuthenticationRequired",
                "Invalid identifier or password",
            )
        };
        let Some(did) = did else {
            return Err(unauthorized());
        };
        let a = &self.accounts[&did];
        let app_password = if a.app_passwords.iter().any(|p| p == password) {
            true
        } else if a.password == password {
            false
        } else {
            return Err(unauthorized());
        };
        if a.status == AccountStatus::Takendown && !allow_takendown {
            return Err(resp::xrpc_error(
                401,
                "AccountTakedown",
                "Account has been taken down",
            ));
        }
        let tokens = self.issue_session(base, &did, app_password, now);
        let json = self.session_json(&did, Some(tokens));
        Ok((did, json))
    }

    pub(crate) fn refresh_session_impl(
        &mut self,
        base: &str,
        userpds: Option<&str>,
        inc: &Incoming<'_>,
        now: i64,
    ) -> Result<(String, Value), HttpResponse> {
        if userpds.is_some() {
            return Err(invalid_token("refreshSession needs a refresh token"));
        }
        let did = self.authenticate(base, None, inc, now, true)?;
        let tok = Self::bearer(inc)?.unwrap_or_default().to_owned();
        let pds = self.pds.get_mut(base).expect("known PDS");
        let app_password = match pds.tokens.get_mut(&tok) {
            Some(t) => {
                t.revoked = true;
                t.app_password
            }
            None => false,
        };
        let tokens = self.issue_session(base, &did, app_password, now);
        let json = self.session_json(&did, Some(tokens));
        Ok((did, json))
    }

    pub(crate) fn get_session_impl(
        &mut self,
        base: &str,
        userpds: Option<&str>,
        inc: &Incoming<'_>,
        now: i64,
    ) -> Result<(String, Value), HttpResponse> {
        let did = self.authenticate(base, userpds, inc, now, false)?;
        let json = self.session_json(&did, None);
        Ok((did, json))
    }

    pub(crate) fn delete_session_impl(
        &mut self,
        base: &str,
        userpds: Option<&str>,
        inc: &Incoming<'_>,
        now: i64,
    ) -> Result<String, HttpResponse> {
        if userpds.is_some() {
            return Err(invalid_token("deleteSession needs a refresh token"));
        }
        let did = self.authenticate(base, None, inc, now, true)?;
        let tok = Self::bearer(inc)?.unwrap_or_default().to_owned();
        if let Some(t) = self.pds.get_mut(base).and_then(|p| p.tokens.get_mut(&tok)) {
            t.revoked = true;
        }
        Ok(did)
    }
}
