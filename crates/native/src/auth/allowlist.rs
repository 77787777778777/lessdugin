// SPDX-License-Identifier: AGPL-3.0-or-later
//! The write allowlist (`threat-model.md` AUTH-4), applied by every native [`AuthSession`]
//! before a request is authenticated:
//!
//! - `GET /xrpc/com.atproto.server.getSession`;
//! - `POST /xrpc/com.atproto.repo.{createRecord, putRecord, deleteRecord, applyWrites}` whose
//!   `repo` is the session's DID and whose `collection` (each op's, for `applyWrites`) is one of
//!   the 10 `NS` record collections;
//! - `POST /xrpc/com.atproto.repo.uploadBlob`.
//!
//! Anything else is refused locally with [`HostError::Blocked`].
//!
//! [`AuthSession`]: crate::host::AuthSession

use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{HostError, HttpRequest, Target};
use lessdugin_lexicon::ns::NS;
use serde_json::Value;

/// The 10 record collections of `NS.authFull` (`data-model.md` §10.1), without the prefix.
pub const COLLECTIONS: [&str; 10] = [
    "forum",
    "membership",
    "post",
    "comment",
    "revision",
    "opinion",
    "modAction",
    "wikiRevision",
    "sequence",
    "profile",
];

/// True for `NS.<collection>` of this build.
pub fn is_ns_collection(nsid: &str) -> bool {
    nsid.strip_prefix(NS)
        .and_then(|r| r.strip_prefix('.'))
        .is_some_and(|c| COLLECTIONS.contains(&c))
}

fn refuse(why: impl Into<String>) -> HostError {
    HostError::Blocked(format!(
        "{} (not on the write allowlist, threat-model AUTH-4)",
        why.into()
    ))
}

fn check_record_op(v: &Value, what: &str) -> Result<(), HostError> {
    let collection = v.get("collection").and_then(Value::as_str).unwrap_or("");
    if !is_ns_collection(collection) {
        return Err(refuse(format!("{what} to collection {collection:?}")));
    }
    Ok(())
}

/// Checks one authenticated request of `account`.
pub fn check(account: &Did, req: &HttpRequest) -> Result<(), HostError> {
    let Target::UserPds { path_and_query, .. } = &req.target else {
        return Err(refuse("an anonymous request"));
    };
    let path = path_and_query.split('?').next().unwrap_or("");
    let nsid = path.strip_prefix("/xrpc/").unwrap_or("");
    match (&req.method, nsid) {
        (m, "com.atproto.server.getSession") if m == http::Method::GET => Ok(()),
        (m, "com.atproto.repo.uploadBlob") if m == http::Method::POST => Ok(()),
        (
            m,
            "com.atproto.repo.createRecord"
            | "com.atproto.repo.putRecord"
            | "com.atproto.repo.deleteRecord"
            | "com.atproto.repo.applyWrites",
        ) if m == http::Method::POST => {
            let body: Value = req
                .body
                .as_deref()
                .and_then(|b| serde_json::from_slice(b).ok())
                .ok_or_else(|| refuse(format!("{nsid} without a JSON body")))?;
            let repo = body.get("repo").and_then(Value::as_str).unwrap_or("");
            if repo != account.as_str() {
                return Err(refuse(format!("{nsid} to repo {repo:?}, not {account}")));
            }
            if nsid == "com.atproto.repo.applyWrites" {
                let writes = body
                    .get("writes")
                    .and_then(Value::as_array)
                    .ok_or_else(|| refuse("applyWrites without writes"))?;
                for w in writes {
                    check_record_op(w, nsid)?;
                }
                Ok(())
            } else {
                check_record_op(&body, nsid)
            }
        }
        (m, _) => Err(refuse(format!("{m} {path}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn did() -> Did {
        Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap()
    }

    fn req(method: http::Method, path: &str, body: Option<Value>) -> HttpRequest {
        let target = Target::UserPds {
            account: did(),
            path_and_query: path.into(),
        };
        let mut r = HttpRequest::get(target);
        r.method = method;
        r.body = body.map(|b| serde_json::to_vec(&b).unwrap());
        r
    }

    #[test]
    fn allowed_requests() {
        let post = format!("{NS}.post");
        let d = did().to_string();
        for r in [
            req(
                http::Method::GET,
                "/xrpc/com.atproto.server.getSession",
                None,
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.uploadBlob",
                None,
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.putRecord",
                Some(json!({"repo": d, "collection": post, "rkey": "x", "record": {}})),
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.applyWrites",
                Some(json!({"repo": d, "writes": [
                    {"$type": "com.atproto.repo.applyWrites#delete", "collection": format!("{NS}.comment"), "rkey": "a"},
                    {"$type": "com.atproto.repo.applyWrites#create", "collection": format!("{NS}.opinion"), "value": {}},
                ]})),
            ),
        ] {
            check(&did(), &r).unwrap();
        }
    }

    #[test]
    fn refused_requests() {
        let d = did().to_string();
        for r in [
            req(
                http::Method::GET,
                "/xrpc/com.atproto.repo.listRecords?repo=x",
                None,
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.server.getSession",
                None,
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.server.deleteAccount",
                None,
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.putRecord",
                Some(json!({"repo": d, "collection": "app.bsky.feed.post", "rkey": "x"})),
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.createRecord",
                Some(json!({"repo": "did:plc:someoneelse", "collection": format!("{NS}.post")})),
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.createRecord",
                Some(json!({"repo": "alice.test", "collection": format!("{NS}.post")})),
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.applyWrites",
                Some(
                    json!({"repo": d, "writes": [{"collection": format!("{NS}.post")}, {"collection": "app.bsky.graph.follow"}]}),
                ),
            ),
            req(
                http::Method::POST,
                "/xrpc/com.atproto.repo.deleteRecord",
                None,
            ),
        ] {
            assert!(
                matches!(check(&did(), &r), Err(HostError::Blocked(_))),
                "{r:?}"
            );
        }
        assert!(!is_ns_collection(&format!("{NS}.postx")));
        assert!(!is_ns_collection(&format!("{NS}post")));
        assert!(is_ns_collection(&format!("{NS}.wikiRevision")));
    }
}
