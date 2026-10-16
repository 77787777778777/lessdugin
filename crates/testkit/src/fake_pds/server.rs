// SPDX-License-Identifier: AGPL-3.0-or-later
//! Request dispatch: service classification, faults, rate limits, the XRPC endpoints, the PLC
//! directory, relays and DoH, and the request log.

use std::collections::BTreeMap;
use std::sync::Arc;

use lessdugin_atp::Cid;
use lessdugin_atp::datamodel::{self, DataModelError};
use lessdugin_atp::syntax::{Nsid, RecordKey};
use lessdugin_core::host::{HostError, HttpResponse};
use serde_json::{Map, Value, json};

use super::blob::sniff_mime;
use super::faults::{CursorMode, Fault, ForkSelector, MatchKey};
use super::log::{LoggedRequest, ServiceKind};
use super::repo::{Rec, Repo};
use super::state::{AccountStatus, NetState, OpaqueCursor, StoredBlob};
use crate::resp;

/// A request as the fake network sees it.
pub(crate) struct Incoming<'a> {
    pub method: &'a http::Method,
    /// Full URL, or `userpds://<did><path_and_query>`.
    pub url: &'a str,
    /// Header names lowercased.
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    pub caller: &'a str,
}

impl Incoming<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// How the transport must deliver an answer.
#[derive(Debug)]
pub(crate) enum Abort {
    /// Drop the connection without an answer.
    Disconnect,
    /// Deliver only this many body bytes.
    Truncate(usize),
    /// A host-level failure (MemHost only).
    Host(HostError),
}

/// The answer and how to deliver it.
pub(crate) struct Outgoing {
    pub resp: HttpResponse,
    pub delay_ms: u64,
    pub abort: Option<Abort>,
}

/// Where a request goes.
struct Route {
    service: ServiceKind,
    base: String,
    path: String,
    params: BTreeMap<String, String>,
    /// The account of a `userpds://` request (authenticated by the host).
    userpds: Option<String>,
}

/// Per-request context for handlers.
struct Ctx<'a> {
    now: i64,
    caller: &'a str,
    fork: bool,
    auth: Option<String>,
}

const PROCEDURES: &[&str] = &[
    "com.atproto.repo.createRecord",
    "com.atproto.repo.putRecord",
    "com.atproto.repo.deleteRecord",
    "com.atproto.repo.applyWrites",
    "com.atproto.repo.uploadBlob",
    "com.atproto.server.createSession",
    "com.atproto.server.refreshSession",
    "com.atproto.server.deleteSession",
];

/// Maximum operations per `applyWrites`.
pub const APPLY_WRITES_MAX: usize = 200;

type R = Result<HttpResponse, HttpResponse>;

fn bad(message: &str) -> HttpResponse {
    resp::xrpc_error(400, "InvalidRequest", message)
}

fn internal() -> HttpResponse {
    resp::xrpc_error(500, "InternalServerError", "Internal Server Error")
}

fn repo_not_found(did: &str) -> HttpResponse {
    resp::xrpc_error(
        400,
        "RepoNotFound",
        &format!("Could not find repo for DID: {did}"),
    )
}

fn status_error(did: &str, status: AccountStatus) -> HttpResponse {
    match status {
        AccountStatus::Deactivated => resp::xrpc_error(
            400,
            "RepoDeactivated",
            &format!("Repo has been deactivated: {did}"),
        ),
        AccountStatus::Takendown => resp::xrpc_error(
            400,
            "RepoTakendown",
            &format!("Repo has been takendown: {did}"),
        ),
        AccountStatus::Suspended => resp::xrpc_error(
            400,
            "RepoSuspended",
            &format!("Repo has been suspended: {did}"),
        ),
        AccountStatus::Active | AccountStatus::Deleted => repo_not_found(did),
    }
}

fn need<'a>(p: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, HttpResponse> {
    p.get(name)
        .map(String::as_str)
        .ok_or_else(|| bad(&format!("Params must have the property \"{name}\"")))
}

fn need_str<'a>(b: &'a Map<String, Value>, name: &str) -> Result<&'a str, HttpResponse> {
    match b.get(name) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(bad(&format!("Input/{name} must be a string"))),
        None => Err(bad(&format!("Input must have the property \"{name}\""))),
    }
}

fn opt_str<'a>(b: &'a Map<String, Value>, name: &str) -> Result<Option<&'a str>, HttpResponse> {
    match b.get(name) {
        Some(Value::String(s)) => Ok(Some(s)),
        None | Some(Value::Null) => Ok(None),
        Some(_) => Err(bad(&format!("Input/{name} must be a string"))),
    }
}

fn opt_bool(b: &Map<String, Value>, name: &str) -> Result<Option<bool>, HttpResponse> {
    match b.get(name) {
        Some(Value::Bool(v)) => Ok(Some(*v)),
        None | Some(Value::Null) => Ok(None),
        Some(_) => Err(bad(&format!("Input/{name} must be a boolean"))),
    }
}

fn check_nsid(s: &str) -> Result<(), HttpResponse> {
    Nsid::parse(s)
        .map(drop)
        .map_err(|_| bad(&format!("Invalid NSID (got \"{s}\") at collection")))
}

fn check_rkey(s: &str) -> Result<(), HttpResponse> {
    RecordKey::parse(s)
        .map(drop)
        .map_err(|_| bad(&format!("Invalid record key (got \"{s}\") at $.rkey")))
}

/// `swapRecord`: absent → `None`; `null` → `Some(None)`; a CID → `Some(Some(cid))`.
fn swap_field(b: &Map<String, Value>, name: &str) -> Result<Option<Option<Cid>>, HttpResponse> {
    match b.get(name) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => lessdugin_atp::cid::parse(s)
            .map(|c| Some(Some(c)))
            .map_err(|_| bad(&format!("Input/{name} must be a cid string"))),
        Some(_) => Err(bad(&format!("Input/{name} must be a cid string"))),
    }
}

fn validation_status(validate: Option<bool>) -> Option<&'static str> {
    match validate {
        Some(false) => None,
        _ => Some("unknown"),
    }
}

/// A record value as the PDS stores it: `$type` checked (or injected), floats rejected.
fn prepare_record(coll: &str, v: &Value, validate: Option<bool>) -> Result<Rec, HttpResponse> {
    let Some(obj) = v.as_object() else {
        return Err(bad("Invalid record: must be an object"));
    };
    let mut obj = obj.clone();
    match obj.get("$type") {
        None => {
            obj.insert("$type".to_owned(), Value::String(coll.to_owned()));
        }
        Some(Value::String(t)) if t == coll => {}
        Some(Value::String(t)) => {
            return Err(bad(&format!("Invalid $type: expected {coll}, got {t}")));
        }
        Some(_) => return Err(bad("Invalid record: $type must be a string")),
    }
    if validate == Some(true) {
        return Err(bad(&format!("Unknown lexicon type: {coll}")));
    }
    let cbor = datamodel::record_to_cbor(&Value::Object(obj)).map_err(|e| match e {
        DataModelError::Float => bad("Invalid record: floats are not allowed"),
        e => bad(&format!("Invalid record: {e}")),
    })?;
    Ok(Rec::from_cbor(cbor))
}

fn commit_json(cid: &Cid, rev: &lessdugin_atp::syntax::Tid) -> Value {
    json!({ "cid": cid.to_string(), "rev": rev.to_string() })
}

fn at_uri(did: &str, coll: &str, rkey: &str) -> String {
    format!("at://{did}/{coll}/{rkey}")
}

fn fault_label(f: &Fault) -> String {
    match f {
        Fault::Latency(ms) => format!("latency {ms}ms"),
        Fault::Status(s) => format!("status {s}"),
        Fault::Error { status, error, .. } => format!("error {status} {error}"),
        Fault::RateLimited { retry_after_s } => format!("429 retry-after {retry_after_s}s"),
        Fault::Truncate(n) => format!("truncate {n}"),
        Fault::Disconnect => "disconnect".to_owned(),
        Fault::Respond(r) => format!("respond {}", r.status),
    }
}

fn fault_response(f: &Fault) -> Result<HttpResponse, Abort> {
    Ok(match f {
        Fault::Status(s) if *s >= 500 => resp::xrpc_error(*s, "InternalServerError", "Fake fault"),
        Fault::Status(s) => resp::xrpc_error(*s, "InvalidRequest", "Fake fault"),
        Fault::Error {
            status,
            error,
            message,
        } => resp::xrpc_error(*status, error, message),
        Fault::RateLimited { retry_after_s } => resp::with_header(
            resp::xrpc_error(429, "RateLimitExceeded", "Rate Limit Exceeded"),
            "retry-after",
            &retry_after_s.to_string(),
        ),
        Fault::Respond(r) => r.clone(),
        Fault::Disconnect => return Err(Abort::Disconnect),
        Fault::Latency(_) | Fault::Truncate(_) => unreachable!("not terminal"),
    })
}

fn parse_query(q: &str) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect()
}

impl NetState {
    // -----------------------------------------------------------------------------------------
    // Dispatch

    fn classify(&self, url: &str) -> Option<Route> {
        if let Some(rest) = url.strip_prefix("userpds://") {
            let cut = rest.find(['/', '?']).unwrap_or(rest.len());
            let did = &rest[..cut];
            let (path, query) = rest[cut..].split_once('?').unwrap_or((&rest[cut..], ""));
            let base = self.accounts.get(did).map(|a| a.pds.clone())?;
            return Some(Route {
                service: ServiceKind::Pds,
                base,
                path: path.to_owned(),
                params: parse_query(query),
                userpds: Some(did.to_owned()),
            });
        }
        let candidates = self
            .pds
            .keys()
            .map(|b| (b.as_str(), ServiceKind::Pds))
            .chain(std::iter::once((
                self.config.plc.as_str(),
                ServiceKind::Plc,
            )))
            .chain(
                self.config
                    .relays
                    .iter()
                    .map(|b| (b.as_str(), ServiceKind::Relay)),
            )
            .chain(
                self.config
                    .doh
                    .iter()
                    .map(|b| (b.as_str(), ServiceKind::Doh)),
            );
        let mut best: Option<(&str, ServiceKind)> = None;
        for (base, kind) in candidates {
            let rest = match url.strip_prefix(base) {
                Some(r) => r,
                None => continue,
            };
            if !(rest.is_empty() || rest.starts_with(['/', '?', '&'])) {
                continue;
            }
            if best.as_ref().is_none_or(|(b, _)| base.len() > b.len()) {
                best = Some((base, kind));
            }
        }
        let (base, service) = best?;
        let rest = &url[base.len()..];
        let (path, query) = match service {
            // DoH endpoints may carry their own query (`?ct=json&name=…`).
            ServiceKind::Doh => ("", url.split_once('?').map_or("", |(_, q)| q)),
            _ => rest.split_once('?').unwrap_or((rest, "")),
        };
        Some(Route {
            service,
            base: base.to_owned(),
            path: path.to_owned(),
            params: parse_query(query),
            userpds: None,
        })
    }

    /// Answers one request at fake time `now`.
    pub fn dispatch(&mut self, inc: &Incoming<'_>, now: i64) -> Outgoing {
        self.request_seq += 1;
        let seq = self.request_seq;
        let Some(route) = self.classify(inc.url) else {
            let (resp, abort) = match inc.url.strip_prefix("userpds://") {
                Some(rest) => {
                    let did = rest.split(['/', '?']).next().unwrap_or_default().to_owned();
                    (
                        resp::status(0),
                        Some(Abort::Host(HostError::AuthRequired(did))),
                    )
                }
                None => (
                    resp::xrpc_error(404, "NotFound", "not on the fake network"),
                    None,
                ),
            };
            self.log.push(LoggedRequest {
                seq,
                at_ms: now,
                caller: inc.caller.to_owned(),
                method: inc.method.clone(),
                url: inc.url.to_owned(),
                service: ServiceKind::Unknown,
                host: String::new(),
                nsid: None,
                params: BTreeMap::new(),
                did: None,
                auth: None,
                status: resp.status,
                error: None,
                faults: Vec::new(),
                fork: false,
            });
            return Outgoing {
                resp,
                delay_ms: 0,
                abort,
            };
        };
        let nsid = match route.service {
            ServiceKind::Plc => Some("plc".to_owned()),
            ServiceKind::Doh => Some("doh".to_owned()),
            _ => route.path.strip_prefix("/xrpc/").map(str::to_owned),
        };
        let is_blob_upload = nsid.as_deref() == Some("com.atproto.repo.uploadBlob");
        let body_json: Option<Value> = if *inc.method == http::Method::POST && !is_blob_upload {
            inc.body.and_then(|b| serde_json::from_slice(b).ok())
        } else {
            None
        };
        let subject = self.subject_did(&route, body_json.as_ref());

        // Faults.
        let key = MatchKey {
            host: &route.base,
            did: subject.as_deref(),
            nsid: nsid.as_deref(),
            caller: inc.caller,
        };
        let mut delay = 0u64;
        let mut truncate = None;
        let mut terminal: Option<Fault> = None;
        let mut labels = Vec::new();
        for f in &mut self.faults {
            if !f.rule.matches(&key) || (terminal.is_some() && f.rule.fault.is_terminal()) {
                continue;
            }
            if !f.hit() {
                continue;
            }
            labels.push(fault_label(&f.rule.fault));
            match &f.rule.fault {
                Fault::Latency(ms) => delay = delay.saturating_add(*ms),
                Fault::Truncate(n) => truncate = Some(*n),
                other => terminal = Some(other.clone()),
            }
        }
        self.faults.retain(|f| !f.expired());

        let mut ctx = Ctx {
            now,
            caller: inc.caller,
            fork: false,
            auth: route.userpds.clone(),
        };
        let mut abort = None;
        let mut extra_headers = Vec::new();
        let resp = match terminal {
            Some(f) => match fault_response(&f) {
                Ok(r) => r,
                Err(a) => {
                    abort = Some(a);
                    resp::status(0)
                }
            },
            None => {
                let limited = match (&route.service, self.pds.get_mut(&route.base)) {
                    (ServiceKind::Pds, Some(p)) => p.rate.as_mut().map(|r| r.take(now)),
                    _ => None,
                };
                match limited {
                    Some((false, h)) => {
                        let mut r =
                            resp::xrpc_error(429, "RateLimitExceeded", "Rate Limit Exceeded");
                        r.headers.extend(h);
                        r
                    }
                    other => {
                        if let Some((_, h)) = other {
                            extra_headers = h;
                        }
                        self.route(&route, nsid.as_deref(), inc, body_json.as_ref(), &mut ctx)
                    }
                }
            }
        };
        let mut resp = resp;
        resp.headers.extend(extra_headers);
        if abort.is_none()
            && let Some(n) = truncate
        {
            abort = Some(Abort::Truncate(n));
        }
        let error = (resp.status >= 400)
            .then(|| {
                serde_json::from_slice::<Value>(&resp.body)
                    .ok()?
                    .get("error")?
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten();
        self.log.push(LoggedRequest {
            seq,
            at_ms: now,
            caller: inc.caller.to_owned(),
            method: inc.method.clone(),
            url: inc.url.to_owned(),
            service: route.service.clone(),
            host: route.base.clone(),
            nsid,
            params: route.params.clone(),
            did: subject,
            auth: ctx.auth.clone(),
            status: if matches!(abort, Some(Abort::Disconnect)) {
                0
            } else {
                resp.status
            },
            error,
            faults: labels,
            fork: ctx.fork,
        });
        Outgoing {
            resp,
            delay_ms: delay,
            abort,
        }
    }

    fn subject_did(&self, route: &Route, body: Option<&Value>) -> Option<String> {
        if route.service == ServiceKind::Plc {
            let p = route.path.trim_start_matches('/');
            let did = p.split('/').next().unwrap_or_default();
            return did.starts_with("did:").then(|| did.to_owned());
        }
        if route.service == ServiceKind::Doh {
            let name = route.params.get("name")?;
            let handle = name.trim_end_matches('.').strip_prefix("_atproto.")?;
            return self.resolve_ident(handle);
        }
        let from_params = route
            .params
            .get("did")
            .or_else(|| route.params.get("repo"))
            .cloned();
        let from_body = || {
            body.and_then(|b| b.get("repo"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        from_params
            .or_else(from_body)
            .and_then(|i| self.resolve_ident(&i))
            .or_else(|| route.userpds.clone())
    }

    fn route(
        &mut self,
        route: &Route,
        nsid: Option<&str>,
        inc: &Incoming<'_>,
        body: Option<&Value>,
        ctx: &mut Ctx<'_>,
    ) -> HttpResponse {
        let r = match route.service {
            ServiceKind::Pds => self.pds_route(route, nsid, inc, body, ctx),
            ServiceKind::Plc => Ok(self.plc(route)),
            ServiceKind::Relay => self.relay(route, nsid),
            ServiceKind::Doh => Ok(self.doh(route)),
            ServiceKind::Unknown => Ok(resp::xrpc_error(404, "NotFound", "Not Found")),
        };
        r.unwrap_or_else(|e| e)
    }

    fn pds_route(
        &mut self,
        route: &Route,
        nsid: Option<&str>,
        inc: &Incoming<'_>,
        body: Option<&Value>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let base = route.base.as_str();
        if route.path == "/.well-known/oauth-protected-resource" {
            let pds = &self.pds[base];
            let servers = pds
                .cfg
                .authorization_servers
                .clone()
                .unwrap_or_else(|| vec![base.to_owned()]);
            return Ok(resp::ok_json(&json!({
                "resource": base,
                "authorization_servers": servers,
                "scopes_supported": [],
                "bearer_methods_supported": ["header"],
            })));
        }
        let Some(nsid) = nsid else {
            return Ok(resp::xrpc_error(404, "NotFound", "Not Found"));
        };
        if nsid == "_health" {
            return Ok(resp::ok_json(&json!({ "version": "lessdugin-fake-pds" })));
        }
        let procedure = PROCEDURES.contains(&nsid);
        let want = if procedure {
            http::Method::POST
        } else {
            http::Method::GET
        };
        if *inc.method != want {
            return Err(bad(&format!(
                "Incorrect HTTP method ({}) expected {want}",
                inc.method
            )));
        }
        if procedure && nsid != "com.atproto.repo.uploadBlob" {
            let limit = self.pds[base].cfg.json_body_limit;
            if inc.body.is_some_and(|b| b.len() > limit) {
                return Err(resp::xrpc_error(
                    413,
                    "PayloadTooLarge",
                    "request entity too large",
                ));
            }
        }
        let p = &route.params;
        match nsid {
            "com.atproto.repo.getRecord" => self.get_record(base, p, ctx),
            "com.atproto.repo.listRecords" => self.list_records(base, p, ctx),
            "com.atproto.repo.describeRepo" => self.describe_repo(base, p, ctx),
            "com.atproto.repo.createRecord" => self.create_record(base, route, inc, body, ctx),
            "com.atproto.repo.putRecord" => self.put_record(base, route, inc, body, ctx),
            "com.atproto.repo.deleteRecord" => self.delete_record(base, route, inc, body, ctx),
            "com.atproto.repo.applyWrites" => self.apply_writes(base, route, inc, body, ctx),
            "com.atproto.repo.uploadBlob" => self.upload_blob(base, route, inc, ctx),
            "com.atproto.sync.getLatestCommit" => self.get_latest_commit(base, p, ctx),
            "com.atproto.sync.getRecord" => self.sync_get_record(base, p, ctx),
            "com.atproto.sync.getRepo" => self.get_repo(base, p, ctx),
            "com.atproto.sync.getRepoStatus" => self.get_repo_status(base, p, ctx),
            "com.atproto.sync.getBlob" => self.get_blob(base, p),
            "com.atproto.sync.listBlobs" => self.list_blobs(base, p),
            "com.atproto.identity.resolveHandle" => self.resolve_handle(p),
            "com.atproto.server.describeServer" => Ok(self.describe_server(base)),
            "com.atproto.server.createSession" => {
                let (did, json) = self.create_session_impl(base, body, ctx.now)?;
                ctx.auth = Some(did);
                Ok(resp::ok_json(&json))
            }
            "com.atproto.server.refreshSession" => {
                let up = route.userpds.as_deref();
                let (did, json) = self.refresh_session_impl(base, up, inc, ctx.now)?;
                ctx.auth = Some(did);
                Ok(resp::ok_json(&json))
            }
            "com.atproto.server.getSession" => {
                let up = route.userpds.as_deref();
                let (did, json) = self.get_session_impl(base, up, inc, ctx.now)?;
                ctx.auth = Some(did);
                Ok(resp::ok_json(&json))
            }
            "com.atproto.server.deleteSession" => {
                let up = route.userpds.as_deref();
                ctx.auth = Some(self.delete_session_impl(base, up, inc, ctx.now)?);
                Ok(resp::status(200))
            }
            other => Ok(resp::xrpc_error(
                501,
                "MethodNotImplemented",
                &format!("Method Not Implemented: {other}"),
            )),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Reads

    /// Checks that `did` is readable at `base` and commits its pending writes.
    fn check_read(&mut self, base: &str, did: &str, now: i64) -> Result<(), HttpResponse> {
        match self.accounts.get(did) {
            Some(a) if a.known_at(base) => match a.status {
                AccountStatus::Active => {}
                s => return Err(status_error(did, s)),
            },
            Some(a) if a.former_pds.iter().any(|p| p == base) => {
                return Err(status_error(did, AccountStatus::Deactivated));
            }
            _ => return Err(repo_not_found(did)),
        }
        self.ensure_commit(did, now);
        Ok(())
    }

    /// Picks the fork or the real repo for this request.
    fn select_view(&mut self, did: &str, ctx: &mut Ctx<'_>) {
        let Some(f) = self.accounts.get_mut(did).and_then(|a| a.fork.as_mut()) else {
            ctx.fork = false;
            return;
        };
        f.requests += 1;
        ctx.fork = match &f.selector {
            ForkSelector::All => true,
            ForkSelector::Callers(c) => c.contains(ctx.caller),
            ForkSelector::Alternate => f.requests % 2 == 1,
        };
    }

    fn view(&self, did: &str, fork: bool) -> &Repo {
        let a = &self.accounts[did];
        match (&a.fork, fork) {
            (Some(f), true) => &f.repo,
            _ => &a.repo,
        }
    }

    fn read_repo(
        &mut self,
        base: &str,
        ident: &str,
        ctx: &mut Ctx<'_>,
    ) -> Result<String, HttpResponse> {
        let did = self
            .resolve_ident(ident)
            .ok_or_else(|| repo_not_found(ident))?;
        self.check_read(base, &did, ctx.now)?;
        self.select_view(&did, ctx);
        Ok(did)
    }

    fn get_record(&mut self, base: &str, p: &BTreeMap<String, String>, ctx: &mut Ctx<'_>) -> R {
        let (repo, coll, rkey) = (need(p, "repo")?, need(p, "collection")?, need(p, "rkey")?);
        check_nsid(coll)?;
        check_rkey(rkey)?;
        let did = self.read_repo(base, repo, ctx)?;
        let view = self.view(&did, ctx.fork);
        let want = p.get("cid").filter(|c| !c.is_empty());
        let uri = at_uri(&did, coll, rkey);
        match view.get(coll, rkey) {
            Some(r) if want.is_none_or(|c| *c == r.cid.to_string()) => Ok(resp::ok_json(&json!({
                "uri": uri,
                "cid": r.cid.to_string(),
                "value": r.json(),
            }))),
            _ => Err(resp::xrpc_error(
                400,
                "RecordNotFound",
                &format!("Could not locate record: {uri}"),
            )),
        }
    }

    fn list_records(&mut self, base: &str, p: &BTreeMap<String, String>, ctx: &mut Ctx<'_>) -> R {
        let (repo, coll) = (need(p, "repo")?, need(p, "collection")?);
        check_nsid(coll)?;
        let limit = match p.get("limit") {
            None => 50,
            Some(s) => {
                let n: i64 = s.parse().map_err(|_| {
                    bad("Invalid com.atproto.repo.listRecords params: limit must be an integer")
                })?;
                if n > 100 {
                    return Err(bad(&format!(
                        "Invalid com.atproto.repo.listRecords params: integer too big (maximum 100, got {n})"
                    )));
                }
                if n < 1 {
                    return Err(bad(&format!(
                        "Invalid com.atproto.repo.listRecords params: integer too small (minimum 1, got {n})"
                    )));
                }
                n as usize
            }
        };
        let ascending = match p.get("reverse").map(String::as_str) {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => {
                return Err(bad(
                    "Invalid com.atproto.repo.listRecords params: reverse must be a boolean",
                ));
            }
        };
        let cursor = p.get("cursor").filter(|c| !c.is_empty()).cloned();
        let did = self.read_repo(base, repo, ctx)?;
        let pds = &self.pds[base];
        let mode = self.accounts[&did]
            .cursor_mode
            .unwrap_or(pds.cfg.cursor_mode);
        let n = pds
            .cfg
            .max_page
            .map_or(limit, |m| limit.min(m.max(1) as usize));
        let view = self.view(&did, ctx.fork);
        let bound = match mode {
            CursorMode::Bound | CursorMode::Stuck => cursor.clone(),
            CursorMode::IgnoresBound => cursor.clone().filter(|c| view.get(coll, c).is_some()),
            CursorMode::Opaque | CursorMode::OpaqueStrict => match &cursor {
                None => None,
                Some(tok) => match pds.cursors.get(tok) {
                    Some(e) if e.did == did && e.collection == coll && e.ascending == ascending => {
                        Some(e.rkey.clone())
                    }
                    _ if mode == CursorMode::OpaqueStrict => {
                        return Err(bad("Malformed cursor"));
                    }
                    _ => None,
                },
            },
        };
        let page: Vec<(String, Cid, Value)> = view
            .list(coll, bound.as_deref(), ascending, n)
            .into_iter()
            .map(|(k, r)| (k.to_owned(), r.cid, r.json()))
            .collect();
        let next = match (page.last(), mode) {
            (None, _) => None,
            (Some((last, ..)), CursorMode::Bound | CursorMode::IgnoresBound) => Some(last.clone()),
            (Some((last, ..)), CursorMode::Stuck) => Some(cursor.unwrap_or_else(|| last.clone())),
            (Some((last, ..)), CursorMode::Opaque | CursorMode::OpaqueStrict) => {
                let tok = self.token("cursor");
                let entry = OpaqueCursor {
                    did: did.clone(),
                    collection: coll.to_owned(),
                    ascending,
                    rkey: last.clone(),
                };
                self.pds
                    .get_mut(base)
                    .expect("known PDS")
                    .cursors
                    .insert(tok.clone(), entry);
                Some(tok)
            }
        };
        let records: Vec<Value> = page
            .into_iter()
            .map(|(k, cid, value)| {
                json!({ "uri": at_uri(&did, coll, &k), "cid": cid.to_string(), "value": value })
            })
            .collect();
        let mut out = Map::new();
        if let Some(c) = next {
            out.insert("cursor".to_owned(), Value::String(c));
        }
        out.insert("records".to_owned(), Value::Array(records));
        Ok(resp::ok_json(&Value::Object(out)))
    }

    fn describe_repo(&mut self, base: &str, p: &BTreeMap<String, String>, ctx: &mut Ctx<'_>) -> R {
        let did = self.read_repo(base, need(p, "repo")?, ctx)?;
        let a = &self.accounts[&did];
        Ok(resp::ok_json(&json!({
            "handle": a.handle,
            "did": did,
            "didDoc": a.doc(),
            "collections": self.view(&did, ctx.fork).collections(),
            "handleIsCorrect": true,
        })))
    }

    fn get_latest_commit(
        &mut self,
        base: &str,
        p: &BTreeMap<String, String>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let did = self.read_repo(base, need(p, "did")?, ctx)?;
        let c = &self.view(&did, ctx.fork).commit;
        Ok(resp::ok_json(&commit_json(&c.cid, &c.rev)))
    }

    fn sync_get_record(
        &mut self,
        base: &str,
        p: &BTreeMap<String, String>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let (d, coll, rkey) = (need(p, "did")?, need(p, "collection")?, need(p, "rkey")?);
        check_nsid(coll)?;
        check_rkey(rkey)?;
        let did = self.read_repo(base, d, ctx)?;
        Ok(resp::car(self.view(&did, ctx.fork).proof_car(coll, rkey)))
    }

    fn get_repo(&mut self, base: &str, p: &BTreeMap<String, String>, ctx: &mut Ctx<'_>) -> R {
        let did = self.read_repo(base, need(p, "did")?, ctx)?;
        Ok(resp::car(self.view(&did, ctx.fork).full_car()))
    }

    fn get_repo_status(
        &mut self,
        base: &str,
        p: &BTreeMap<String, String>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let did = need(p, "did")?.to_owned();
        let status = match self.accounts.get(&did) {
            Some(a) if a.known_at(base) => a.status,
            Some(a) if a.former_pds.iter().any(|b| b == base) => AccountStatus::Deactivated,
            _ => return Err(repo_not_found(&did)),
        };
        let mut out = Map::new();
        out.insert("did".to_owned(), Value::String(did.clone()));
        out.insert(
            "active".to_owned(),
            Value::Bool(status == AccountStatus::Active),
        );
        match status.as_status() {
            Some(s) => {
                out.insert("status".to_owned(), Value::String(s.to_owned()));
            }
            None => {
                self.ensure_commit(&did, ctx.now);
                self.select_view(&did, ctx);
                let rev = self.view(&did, ctx.fork).commit.rev.to_string();
                out.insert("rev".to_owned(), Value::String(rev));
            }
        }
        Ok(resp::ok_json(&Value::Object(out)))
    }

    fn get_blob(&mut self, base: &str, p: &BTreeMap<String, String>) -> R {
        let (did, cid) = (need(p, "did")?, need(p, "cid")?);
        let cid = lessdugin_atp::cid::parse(cid).map_err(|_| bad("Invalid cid"))?;
        match self.accounts.get(did) {
            Some(a) if a.known_at(base) && a.status == AccountStatus::Active => {}
            Some(a) if a.known_at(base) => return Err(status_error(did, a.status)),
            _ => return Err(repo_not_found(did)),
        }
        let a = &self.accounts[did];
        match a.blobs.get(&cid).filter(|b| b.permanent) {
            Some(b) => {
                let mut r = resp::bytes(200, &b.mime, b.bytes.as_ref().clone());
                r.headers
                    .push(("x-content-type-options".to_owned(), "nosniff".to_owned()));
                r.headers.push((
                    "content-security-policy".to_owned(),
                    "default-src 'none'; sandbox".to_owned(),
                ));
                Ok(r)
            }
            None => Err(bad("Blob not found")),
        }
    }

    fn list_blobs(&mut self, base: &str, p: &BTreeMap<String, String>) -> R {
        let did = need(p, "did")?;
        match self.accounts.get(did) {
            Some(a) if a.known_at(base) && a.status == AccountStatus::Active => {}
            Some(a) if a.known_at(base) => return Err(status_error(did, a.status)),
            _ => return Err(repo_not_found(did)),
        }
        let limit: usize = match p.get("limit") {
            None => 500,
            Some(s) => s
                .parse()
                .ok()
                .filter(|n| (1..=1000).contains(n))
                .ok_or_else(|| bad("limit must be an integer between 1 and 1000"))?,
        };
        let cursor = p.get("cursor").filter(|c| !c.is_empty());
        let mut cids: Vec<String> = self.accounts[did]
            .blobs
            .iter()
            .filter(|(_, b)| b.permanent)
            .map(|(c, _)| c.to_string())
            .collect();
        cids.sort();
        let page: Vec<String> = cids
            .into_iter()
            .filter(|c| cursor.is_none_or(|cur| c.as_str() > cur.as_str()))
            .take(limit)
            .collect();
        let mut out = Map::new();
        if page.len() == limit
            && let Some(last) = page.last()
        {
            out.insert("cursor".to_owned(), Value::String(last.clone()));
        }
        out.insert("cids".to_owned(), json!(page));
        Ok(resp::ok_json(&Value::Object(out)))
    }

    fn resolve_handle(&mut self, p: &BTreeMap<String, String>) -> R {
        let h = need(p, "handle")?;
        match self
            .resolve_ident(h)
            .filter(|d| !h.starts_with("did:") && !d.is_empty())
        {
            Some(did) => Ok(resp::ok_json(&json!({ "did": did }))),
            None => Err(resp::xrpc_error(
                400,
                "HandleNotFound",
                "Unable to resolve handle",
            )),
        }
    }

    fn describe_server(&self, base: &str) -> HttpResponse {
        let pds = &self.pds[base];
        resp::ok_json(&json!({
            "did": pds.server_did,
            "availableUserDomains": pds.cfg.available_user_domains,
            "inviteCodeRequired": false,
            "phoneVerificationRequired": false,
            "links": {},
            "contact": {},
        }))
    }

    // -----------------------------------------------------------------------------------------
    // Writes

    /// Authenticates a write and returns `(did, body)`; the body's `repo` must be the session's.
    fn write_prelude<'b>(
        &mut self,
        base: &str,
        route: &Route,
        inc: &Incoming<'_>,
        body: Option<&'b Value>,
        ctx: &mut Ctx<'_>,
    ) -> Result<(String, &'b Map<String, Value>), HttpResponse> {
        let did = self.authenticate(base, route.userpds.as_deref(), inc, ctx.now, false)?;
        ctx.auth = Some(did.clone());
        let Some(b) = body.and_then(Value::as_object) else {
            return Err(bad("Request body must be a JSON object"));
        };
        let repo = need_str(b, "repo")?;
        if self.resolve_ident(repo).as_deref() != Some(did.as_str()) {
            return Err(resp::xrpc_error(
                401,
                "AuthenticationRequired",
                "Authentication Required",
            ));
        }
        let status = self.accounts[&did].status;
        if status != AccountStatus::Active {
            return Err(status_error(&did, status));
        }
        self.ensure_commit(&did, ctx.now);
        Ok((did, b))
    }

    fn check_swap_commit(&self, did: &str, b: &Map<String, Value>) -> Result<(), HttpResponse> {
        if let Some(Some(want)) = swap_field(b, "swapCommit")? {
            let cur = self.accounts[did].repo.commit.cid;
            if want != cur {
                return Err(resp::xrpc_error(
                    400,
                    "InvalidSwap",
                    &format!("Commit was at {cur}"),
                ));
            }
        }
        Ok(())
    }

    fn check_blobs<'r>(
        &self,
        did: &str,
        recs: impl IntoIterator<Item = &'r Rec>,
    ) -> Result<(), HttpResponse> {
        let a = &self.accounts[did];
        for r in recs {
            for b in &r.blobs {
                let Some(stored) = a.blobs.get(&b.cid) else {
                    return Err(resp::xrpc_error(
                        400,
                        "BlobNotFound",
                        &format!("Could not find blob: {}", b.cid),
                    ));
                };
                if stored.mime != b.mime_type {
                    return Err(resp::xrpc_error(
                        400,
                        "InvalidMimeType",
                        &format!(
                            "Referenced Mimetype does not match stored blob. Expected: {}, Got: {}",
                            stored.mime, b.mime_type
                        ),
                    ));
                }
                let size = stored.bytes.len() as i64;
                if size != b.size {
                    return Err(resp::xrpc_error(
                        400,
                        "InvalidSize",
                        &format!(
                            "Referenced Size does not match stored blob. Expected: {size}, Got: {}",
                            b.size
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Commits the account's writes now; returns `{cid, rev}`.
    fn commit_now(&mut self, did: &str, now: i64) -> Value {
        if let Some(a) = self.accounts.get_mut(did) {
            a.repo.force_commit();
        }
        self.ensure_commit(did, now);
        let c = &self.accounts[did].repo.commit;
        commit_json(&c.cid, &c.rev)
    }

    fn create_record(
        &mut self,
        base: &str,
        route: &Route,
        inc: &Incoming<'_>,
        body: Option<&Value>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let (did, b) = self.write_prelude(base, route, inc, body, ctx)?;
        let coll = need_str(b, "collection")?;
        check_nsid(coll)?;
        let rkey = opt_str(b, "rkey")?;
        if let Some(k) = rkey {
            check_rkey(k)?;
        }
        let validate = opt_bool(b, "validate")?;
        let record = b
            .get("record")
            .ok_or_else(|| bad("Input must have the property \"record\""))?;
        let rec = prepare_record(coll, record, validate)?;
        self.check_swap_commit(&did, b)?;
        self.check_blobs(&did, [&rec])?;
        let a = self.accounts.get_mut(&did).expect("account");
        let rkey = match rkey {
            Some(k) => k.to_owned(),
            None => a.next_tid(ctx.now).to_string(),
        };
        if a.repo.get(coll, &rkey).is_some() {
            // The reference PDS fails with a generic 500 ("There is already a value at key").
            return Err(internal());
        }
        let cid = a.repo.put(coll, &rkey, rec);
        let commit = self.commit_now(&did, ctx.now);
        let mut out = json!({
            "uri": at_uri(&did, coll, &rkey),
            "cid": cid.to_string(),
            "commit": commit,
        });
        if let Some(s) = validation_status(validate) {
            out["validationStatus"] = json!(s);
        }
        Ok(resp::ok_json(&out))
    }

    fn put_record(
        &mut self,
        base: &str,
        route: &Route,
        inc: &Incoming<'_>,
        body: Option<&Value>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let (did, b) = self.write_prelude(base, route, inc, body, ctx)?;
        let coll = need_str(b, "collection")?;
        check_nsid(coll)?;
        let rkey = need_str(b, "rkey")?;
        check_rkey(rkey)?;
        let validate = opt_bool(b, "validate")?;
        let record = b
            .get("record")
            .ok_or_else(|| bad("Input must have the property \"record\""))?;
        let rec = prepare_record(coll, record, validate)?;
        let swap_record = swap_field(b, "swapRecord")?;
        self.check_swap_commit(&did, b)?;
        let cur = self.accounts[&did].repo.get(coll, rkey).map(|r| r.cid);
        let was_at = || {
            resp::xrpc_error(
                400,
                "InvalidSwap",
                &format!(
                    "Record was at {}",
                    cur.map_or_else(|| "null".to_owned(), |c| c.to_string())
                ),
            )
        };
        match swap_record {
            None => {}
            Some(None) if cur.is_some() => return Err(was_at()),
            Some(Some(want)) if cur != Some(want) => return Err(was_at()),
            Some(_) => {}
        }
        let uri = at_uri(&did, coll, rkey);
        let mut out = json!({ "uri": uri, "cid": rec.cid.to_string() });
        if cur == Some(rec.cid) {
            // An identical value: no commit, no `commit` field (ADR 0005).
            if let Some(s) = validation_status(validate) {
                out["validationStatus"] = json!(s);
            }
            return Ok(resp::ok_json(&out));
        }
        self.check_blobs(&did, [&rec])?;
        let a = self.accounts.get_mut(&did).expect("account");
        a.repo.put(coll, rkey, rec);
        out["commit"] = self.commit_now(&did, ctx.now);
        if let Some(s) = validation_status(validate) {
            out["validationStatus"] = json!(s);
        }
        Ok(resp::ok_json(&out))
    }

    fn delete_record(
        &mut self,
        base: &str,
        route: &Route,
        inc: &Incoming<'_>,
        body: Option<&Value>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let (did, b) = self.write_prelude(base, route, inc, body, ctx)?;
        let coll = need_str(b, "collection")?;
        check_nsid(coll)?;
        let rkey = need_str(b, "rkey")?;
        check_rkey(rkey)?;
        let swap_record = swap_field(b, "swapRecord")?;
        let Some(cur) = self.accounts[&did].repo.get(coll, rkey).map(|r| r.cid) else {
            // Absent: 200 {} without checking any swap (ADR 0005).
            return Ok(resp::ok_json(&json!({})));
        };
        self.check_swap_commit(&did, b)?;
        if let Some(want) = swap_record
            && want != Some(cur)
        {
            return Err(resp::xrpc_error(
                400,
                "InvalidSwap",
                &format!("Record was at {cur}"),
            ));
        }
        self.accounts
            .get_mut(&did)
            .expect("account")
            .repo
            .delete(coll, rkey);
        let commit = self.commit_now(&did, ctx.now);
        Ok(resp::ok_json(&json!({ "commit": commit })))
    }

    fn apply_writes(
        &mut self,
        base: &str,
        route: &Route,
        inc: &Incoming<'_>,
        body: Option<&Value>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        #[derive(PartialEq)]
        enum Kind {
            Create,
            Update,
            Delete,
        }
        let (did, b) = self.write_prelude(base, route, inc, body, ctx)?;
        let validate = opt_bool(b, "validate")?;
        let writes = match b.get("writes") {
            Some(Value::Array(w)) => w,
            Some(_) => return Err(bad("Input/writes must be an array")),
            None => return Err(bad("Input must have the property \"writes\"")),
        };
        if writes.len() > APPLY_WRITES_MAX {
            return Err(bad(&format!("Too many writes. Max: {APPLY_WRITES_MAX}")));
        }
        let mut ops: Vec<(Kind, String, Option<String>, Option<Rec>)> = Vec::new();
        for (i, w) in writes.iter().enumerate() {
            let Some(w) = w.as_object() else {
                return Err(bad(&format!("Input/writes/{i} must be an object")));
            };
            let kind = match w.get("$type").and_then(Value::as_str) {
                Some("com.atproto.repo.applyWrites#create") => Kind::Create,
                Some("com.atproto.repo.applyWrites#update") => Kind::Update,
                Some("com.atproto.repo.applyWrites#delete") => Kind::Delete,
                _ => {
                    return Err(bad(&format!(
                        "Input/writes/{i} must be an object which includes the \"$type\" property"
                    )));
                }
            };
            let coll = need_str(w, "collection")?;
            check_nsid(coll)?;
            let rkey = match kind {
                Kind::Create => opt_str(w, "rkey")?,
                _ => Some(need_str(w, "rkey")?),
            };
            if let Some(k) = rkey {
                check_rkey(k)?;
            }
            let rec = match kind {
                Kind::Delete => None,
                _ => {
                    let v = w.get("value").ok_or_else(|| {
                        bad(&format!(
                            "Input/writes/{i} must have the property \"value\""
                        ))
                    })?;
                    Some(prepare_record(coll, v, validate)?)
                }
            };
            ops.push((kind, coll.to_owned(), rkey.map(str::to_owned), rec));
        }
        self.check_swap_commit(&did, b)?;
        self.check_blobs(&did, ops.iter().filter_map(|o| o.3.as_ref()))?;
        let a = self.accounts.get_mut(&did).expect("account");
        for op in &mut ops {
            if op.2.is_none() {
                op.2 = Some(a.next_tid(ctx.now).to_string());
            }
        }
        // Strict and atomic: simulate first (later ops see earlier ones), then apply.
        let mut overlay: BTreeMap<String, bool> = BTreeMap::new();
        for (kind, coll, rkey, _) in &ops {
            let rkey = rkey.as_deref().unwrap_or_default();
            let path = super::repo::path(coll, rkey);
            let exists = overlay
                .get(&path)
                .copied()
                .unwrap_or_else(|| a.repo.get(coll, rkey).is_some());
            match kind {
                Kind::Create if exists => return Err(internal()),
                Kind::Update | Kind::Delete if !exists => return Err(internal()),
                _ => {}
            }
            overlay.insert(path, *kind != Kind::Delete);
        }
        let status = validation_status(validate);
        let mut results = Vec::new();
        for (kind, coll, rkey, rec) in ops {
            let rkey = rkey.unwrap_or_default();
            let uri = at_uri(&did, &coll, &rkey);
            match (kind, rec) {
                (Kind::Delete, _) => {
                    a.repo.delete(&coll, &rkey);
                    results.push(json!({ "$type": "com.atproto.repo.applyWrites#deleteResult" }));
                }
                (kind, Some(rec)) => {
                    let cid = a.repo.put(&coll, &rkey, rec);
                    let t = if kind == Kind::Create {
                        "com.atproto.repo.applyWrites#createResult"
                    } else {
                        "com.atproto.repo.applyWrites#updateResult"
                    };
                    let mut r = json!({ "$type": t, "uri": uri, "cid": cid.to_string() });
                    if let Some(s) = status {
                        r["validationStatus"] = json!(s);
                    }
                    results.push(r);
                }
                (_, None) => unreachable!("creates and updates carry a record"),
            }
        }
        let commit = self.commit_now(&did, ctx.now);
        Ok(resp::ok_json(
            &json!({ "commit": commit, "results": results }),
        ))
    }

    fn upload_blob(
        &mut self,
        base: &str,
        route: &Route,
        inc: &Incoming<'_>,
        ctx: &mut Ctx<'_>,
    ) -> R {
        let did = self.authenticate(base, route.userpds.as_deref(), inc, ctx.now, false)?;
        ctx.auth = Some(did.clone());
        let status = self.accounts[&did].status;
        if status != AccountStatus::Active {
            return Err(status_error(&did, status));
        }
        let bytes = inc.body.unwrap_or_default();
        if bytes.len() as u64 > self.pds[base].cfg.blob_upload_limit {
            return Err(resp::xrpc_error(
                413,
                "PayloadTooLarge",
                "request entity too large",
            ));
        }
        let declared = inc.header("content-type").unwrap_or_default();
        let mime = sniff_mime(bytes, declared);
        let cid = lessdugin_atp::cid::for_raw(bytes);
        let a = self.accounts.get_mut(&did).expect("account");
        let stored = a.blobs.entry(cid).or_insert_with(|| StoredBlob {
            bytes: Arc::new(bytes.to_vec()),
            mime: mime.clone(),
            permanent: false,
        });
        Ok(resp::ok_json(&json!({
            "blob": {
                "$type": "blob",
                "ref": { "$link": cid.to_string() },
                "mimeType": stored.mime,
                "size": bytes.len(),
            }
        })))
    }

    // -----------------------------------------------------------------------------------------
    // PLC, relays, DoH

    fn plc(&self, route: &Route) -> HttpResponse {
        let p = route.path.trim_start_matches('/');
        if p == "_health" {
            return resp::ok_json(&json!({ "version": "lessdugin-fake-plc" }));
        }
        let did = p.split('/').next().unwrap_or_default();
        match self.accounts.get(did) {
            Some(a) if a.plc_tombstoned => resp::json(
                410,
                &json!({ "message": format!("DID not available: {did}") }),
            ),
            Some(a) if p == did => {
                let mut r = resp::ok_json(&a.doc());
                r.headers = vec![(
                    "content-type".to_owned(),
                    "application/did+ld+json; charset=utf-8".to_owned(),
                )];
                r
            }
            _ => resp::json(
                404,
                &json!({ "message": format!("DID not registered: {did}") }),
            ),
        }
    }

    fn relay(&self, route: &Route, nsid: Option<&str>) -> R {
        if nsid != Some("com.atproto.sync.listReposByCollection") {
            return Ok(resp::xrpc_error(
                501,
                "MethodNotImplemented",
                "Method Not Implemented",
            ));
        }
        let p = &route.params;
        let coll = need(p, "collection")?;
        check_nsid(coll)?;
        let limit: usize = match p.get("limit") {
            None => 500,
            Some(s) => s
                .parse()
                .ok()
                .filter(|n| (1..=2000).contains(n))
                .ok_or_else(|| bad("limit must be an integer between 1 and 2000"))?,
        };
        let cursor = p.get("cursor").filter(|c| !c.is_empty());
        let mut dids: Vec<&str> = self
            .accounts
            .values()
            .filter(|a| a.status == AccountStatus::Active && a.repo.has_collection(coll))
            .map(|a| a.did.as_str())
            .filter(|d| cursor.is_none_or(|c| *d > c.as_str()))
            .collect();
        dids.sort_unstable();
        let more = dids.len() > limit;
        dids.truncate(limit);
        let mut out = Map::new();
        if more && let Some(last) = dids.last() {
            out.insert("cursor".to_owned(), Value::String((*last).to_owned()));
        }
        out.insert(
            "repos".to_owned(),
            Value::Array(dids.iter().map(|d| json!({ "did": d })).collect()),
        );
        Ok(resp::ok_json(&Value::Object(out)))
    }

    fn doh(&self, route: &Route) -> HttpResponse {
        let name = route.params.get("name").cloned().unwrap_or_default();
        let qname = name.trim_end_matches('.');
        let did = qname
            .strip_prefix("_atproto.")
            .and_then(|h| self.resolve_ident(h).filter(|_| !h.starts_with("did:")));
        match did {
            Some(d) => resp::ok_json(&crate::identity::doh_txt_answer(
                qname,
                &[&format!("did={d}")],
            )),
            None => resp::ok_json(&crate::identity::doh_nxdomain(qname)),
        }
    }
}
