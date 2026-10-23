// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /bridge/xrpc/<nsid>?account=<did>` (`threat-model.md` §7.4, AUTH-4).
//!
//! One XRPC call of the SPA, sent through the CLI's own session for `<did>` (OAuth with DPoP or
//! app password, from the keyring; the browser never sees a token):
//!
//! - `<nsid>` must be on the AUTH-4 allowlist: `com.atproto.repo.{createRecord, putRecord,
//!   deleteRecord, applyWrites, uploadBlob}` and `com.atproto.server.getSession` (sent to the
//!   PDS as a GET; the bridge takes POST only);
//! - `<did>` must be a logged-in account with a stored session bound to its PDS
//!   ([`restore_session`], AUTH-8);
//! - JSON bodies are parsed and re-serialised (so the PDS sees exactly what was checked), and
//!   the AUTH-4 `repo` and `collection` checks run on them here
//!   ([`lessdugin_native::auth::allowlist::check`]), whatever the SPA checked; the session runs
//!   them once more before it signs;
//! - `uploadBlob` bodies are raw; the declared type must be a `data-model.md` §8.8 type that
//!   the content sniffs as (spilled `text/markdown` must be UTF-8);
//! - the answer is the PDS's status and body with only `Content-Type`, `Retry-After` and
//!   `RateLimit-*` passed through, marked `X-Lessdugin-Upstream: pds`.

use axum::body::{Body, Bytes};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use lessdugin_atp::syntax::Did;
use lessdugin_core::engine::{EngineError, ErrorCode};
use lessdugin_core::host::{Host as _, HostError, HttpRequest, Target};
use lessdugin_core::limits;
use lessdugin_core::outbox::blob::{TEXT_MARKDOWN, normalise_mime};
use lessdugin_native::auth::{allowlist, restore_session};

use super::server::{Web, engine_error, error};

/// `com.atproto.repo.uploadBlob`.
pub const UPLOAD_BLOB: &str = "com.atproto.repo.uploadBlob";
/// `com.atproto.server.getSession`.
pub const GET_SESSION: &str = "com.atproto.server.getSession";

/// The AUTH-4 methods.
pub const ALLOWED: [&str; 6] = [
    "com.atproto.repo.createRecord",
    "com.atproto.repo.putRecord",
    "com.atproto.repo.deleteRecord",
    "com.atproto.repo.applyWrites",
    UPLOAD_BLOB,
    GET_SESSION,
];

/// The declared upload types of `data-model.md` §8.8.
pub const DECLARED_TYPES: [&str; 11] = [
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
    "audio/mp4",
    "video/mp4",
    "video/webm",
    "audio/mpeg",
    "audio/ogg",
    "application/pdf",
    TEXT_MARKDOWN,
];

/// The response header marking an answer that comes from the PDS (not from the bridge).
pub const UPSTREAM_HEADER: &str = "x-lessdugin-upstream";

/// The `account` query parameter: exactly one, a DID.
fn account_param(query: Option<&str>) -> Result<Did, &'static str> {
    let mut found = None;
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        if k == "account" {
            if found.is_some() {
                return Err("account is given more than once");
            }
            found = Some(v.into_owned());
        }
    }
    let v = found.ok_or("the account parameter (a DID) is missing")?;
    Did::parse(&v).map_err(|_| "account must be a DID")
}

/// Checks an `uploadBlob` body against its declared type.
fn blob_matches(declared: &str, body: &[u8]) -> bool {
    if declared == TEXT_MARKDOWN {
        return std::str::from_utf8(body).is_ok();
    }
    normalise_mime(body) == Some(declared)
}

/// The bridge answer for a host error.
fn host_error(e: HostError) -> Response {
    let status = match &e {
        HostError::AuthRequired(_) | HostError::Blocked(_) => StatusCode::FORBIDDEN,
        HostError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        HostError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        HostError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::BAD_GATEWAY,
    };
    let e = EngineError::from(e);
    let mut r = engine_error(&e);
    *r.status_mut() = status;
    r
}

/// Forwards one call (the request already passed the §7.3 checks).
pub async fn forward(
    web: &Web,
    nsid: &str,
    query: Option<&str>,
    content_type: &str,
    body: Bytes,
) -> Response {
    if !ALLOWED.contains(&nsid) {
        return error(
            StatusCode::FORBIDDEN,
            "blocked",
            &format!("{nsid} is not on the write allowlist (threat-model AUTH-4)"),
        );
    }
    let did = match account_param(query) {
        Ok(d) => d,
        Err(m) => return error(StatusCode::BAD_REQUEST, "invalid_input", m),
    };
    let engine = &web.engine;
    let accounts = match engine.accounts().await {
        Ok(a) => a,
        Err(e) => return engine_error(&e),
    };
    let Some(account) = accounts.into_iter().find(|a| a.did == did.as_str()) else {
        // Logged out elsewhere: forget any session this process still holds.
        engine.host().remove_session(&did);
        return engine_error(
            &EngineError::new(
                ErrorCode::AuthRequired,
                format!("{did} is not logged in here: run `lessdugin login {did}` in a terminal"),
            )
            .with_detail("did", did.as_str()),
        );
    };
    if engine.host().session(&did).is_none()
        && let Err(e) = restore_session(engine, &web.store, &web.paths, &account).await
    {
        return engine_error(&e);
    }
    let target = Target::UserPds {
        account: did.clone(),
        path_and_query: format!("/xrpc/{nsid}"),
    };
    let req = if nsid == GET_SESSION {
        HttpRequest::get(target).with_max_bytes(limits::SMALL_RESPONSE_MAX_BYTES)
    } else if nsid == UPLOAD_BLOB {
        if !blob_matches(content_type, &body) {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_input",
                "the blob's content is not of its declared type (data-model.md 8.8)",
            );
        }
        let mut r = HttpRequest::post_json(target, body.to_vec())
            .with_max_bytes(limits::SMALL_RESPONSE_MAX_BYTES)
            .with_timeout_ms(limits::BLOB_TOTAL_TIMEOUT_MS);
        r.headers = vec![("content-type".into(), content_type.to_owned())];
        r
    } else {
        let value: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v @ serde_json::Value::Object(_)) => v,
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "the body must be a JSON object",
                );
            }
        };
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        HttpRequest::post_json(target, bytes)
            .with_max_bytes(limits::DEFAULT_RESPONSE_MAX_BYTES)
            .with_timeout_ms(limits::REQUEST_TIMEOUT_MS)
    };
    // AUTH-4 on the parsed body (repo is the account, every collection is ours).
    if let Err(e) = allowlist::check(&did, &req) {
        return host_error(e);
    }
    let resp = match engine.host().http(req).await {
        Ok(r) => r,
        Err(e) => return host_error(e),
    };
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = (status, Body::from(resp.body)).into_response();
    let h = out.headers_mut();
    for (k, v) in &resp.headers {
        let k = k.to_ascii_lowercase();
        let pass = k == "content-type" || k == "retry-after" || k.starts_with("ratelimit-");
        if !pass {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            h.append(name, value);
        }
    }
    if !h.contains_key(header::CONTENT_TYPE) {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
    }
    h.insert(
        HeaderName::from_static(UPSTREAM_HEADER),
        HeaderValue::from_static("pds"),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_parameter() {
        let did = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
        assert_eq!(
            account_param(Some(&format!("account={did}")))
                .unwrap()
                .as_str(),
            did
        );
        assert_eq!(
            account_param(Some("account=did%3Aplc%3Aewvi7nxzyoun6zhxrhs64oiz"))
                .unwrap()
                .as_str(),
            did
        );
        assert!(account_param(None).is_err());
        assert!(account_param(Some("account=alice.test")).is_err());
        assert!(account_param(Some(&format!("account={did}&account={did}"))).is_err());
    }

    #[test]
    fn blob_types() {
        assert!(blob_matches("image/png", b"\x89PNG\r\n\x1a\n...."));
        assert!(!blob_matches("image/jpeg", b"\x89PNG\r\n\x1a\n...."));
        assert!(!blob_matches("image/png", b"<svg/>"));
        assert!(blob_matches("text/markdown", "# hi".as_bytes()));
        assert!(!blob_matches("text/markdown", b"\xff\xfe"));
        for t in DECLARED_TYPES {
            assert_eq!(t, t.to_ascii_lowercase());
        }
    }
}
