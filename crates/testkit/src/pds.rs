// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`FakePds`]: the read side of a PDS, served from [`TestRepo`]s on a [`MemHost`].
//!
//! Routes under `<base>/xrpc/`: `com.atproto.sync.getRecord` (proof CAR, presence or absence),
//! `com.atproto.repo.getRecord` (JSON or `RecordNotFound`), `com.atproto.sync.getLatestCommit`,
//! `com.atproto.identity.resolveHandle`; and `<base>/.well-known/oauth-protected-resource`.
//! Unknown repos answer `RepoNotFound`, other methods `MethodNotImplemented`. Faults are injected
//! by registering a longer route prefix on the host (it wins over the PDS's routes).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use lessdugin_atp::syntax::{Did, Tid};
use lessdugin_core::host::{HostError, HttpRequest, HttpResponse};
use serde_json::json;

use crate::memhost::{MemHost, target_url};
use crate::repo::TestRepo;
use crate::resp;

struct RepoEntry {
    repo: TestRepo,
    rev: Tid,
}

#[derive(Default)]
struct State {
    repos: BTreeMap<String, RepoEntry>,
    handles: BTreeMap<String, String>,
    authorization_servers: Option<Vec<String>>,
}

/// A fake PDS. Cloning shares the state.
#[derive(Clone)]
pub struct FakePds {
    base: String,
    state: Arc<Mutex<State>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakePds {
    /// Installs a PDS at `base` (e.g. `https://pds.example.com`) on `host`. Its authorization
    /// server defaults to `base` itself.
    pub fn install(host: &MemHost, base: &str) -> Self {
        let pds = Self {
            base: base.trim_end_matches('/').to_owned(),
            state: Arc::default(),
        };
        let p = pds.clone();
        host.on_get(&format!("{}/xrpc/", pds.base), move |req| {
            p.handle_xrpc(req)
        });
        let p = pds.clone();
        host.on_get(
            &format!("{}/.well-known/oauth-protected-resource", pds.base),
            move |_| {
                let servers = lock(&p.state)
                    .authorization_servers
                    .clone()
                    .unwrap_or_else(|| vec![p.base.clone()]);
                Ok(resp::ok_json(&json!({
                    "resource": p.base,
                    "authorization_servers": servers,
                    "scopes_supported": [],
                    "bearer_methods_supported": ["header"],
                })))
            },
        );
        pds
    }

    /// The base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Hosts a repository at revision `rev`.
    pub fn add_repo(&self, repo: TestRepo, rev: Tid) {
        lock(&self.state)
            .repos
            .insert(repo.did().to_string(), RepoEntry { repo, rev });
    }

    /// Changes a hosted repository (and optionally its revision).
    pub fn update<R>(&self, did: &Did, rev: Option<Tid>, f: impl FnOnce(&mut TestRepo) -> R) -> R {
        let mut st = lock(&self.state);
        let e = st.repos.get_mut(did.as_str()).expect("hosted repo");
        if let Some(rev) = rev {
            e.rev = rev;
        }
        f(&mut e.repo)
    }

    /// Maps a handle for `resolveHandle`.
    pub fn set_handle(&self, handle: &str, did: &Did) {
        lock(&self.state)
            .handles
            .insert(handle.to_ascii_lowercase(), did.to_string());
    }

    /// Sets `authorization_servers` in the protected-resource metadata.
    pub fn set_authorization_servers(&self, servers: Vec<String>) {
        lock(&self.state).authorization_servers = Some(servers);
    }

    fn handle_xrpc(&self, req: &HttpRequest) -> Result<HttpResponse, HostError> {
        let url = url::Url::parse(&target_url(&req.target))
            .map_err(|e| HostError::Other(e.to_string()))?;
        let q: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        let arg = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
        let nsid = url.path().trim_start_matches("/xrpc/");
        let st = lock(&self.state);
        let repo = |did: &str| st.repos.get(did);
        let not_found = || resp::xrpc_error(400, "RepoNotFound", "Could not find repo");
        Ok(match nsid {
            "com.atproto.sync.getRecord" => match repo(arg("did")) {
                Some(e) => resp::car(e.repo.proof_car(arg("collection"), arg("rkey"), e.rev)),
                None => not_found(),
            },
            "com.atproto.repo.getRecord" => match repo(arg("repo")) {
                Some(e) => match (
                    e.repo.get(arg("collection"), arg("rkey")),
                    e.repo.cid(arg("collection"), arg("rkey")),
                ) {
                    (Some(value), Some(cid)) => resp::ok_json(&json!({
                        "uri": format!("at://{}/{}/{}", arg("repo"), arg("collection"), arg("rkey")),
                        "cid": cid.to_string(),
                        "value": value,
                    })),
                    _ => resp::xrpc_error(400, "RecordNotFound", "Could not locate record"),
                },
                None => not_found(),
            },
            "com.atproto.sync.getLatestCommit" => match repo(arg("did")) {
                Some(e) => {
                    let c = e.repo.commit(e.rev);
                    resp::ok_json(&json!({ "cid": c.cid.to_string(), "rev": e.rev.to_string() }))
                }
                None => not_found(),
            },
            "com.atproto.identity.resolveHandle" => {
                match st.handles.get(&arg("handle").to_ascii_lowercase()) {
                    Some(did) => resp::ok_json(&json!({ "did": did })),
                    None => resp::xrpc_error(400, "HandleNotFound", "Unable to resolve handle"),
                }
            }
            other => resp::xrpc_error(
                501,
                "MethodNotImplemented",
                &format!("{other} is not implemented"),
            ),
        })
    }
}
