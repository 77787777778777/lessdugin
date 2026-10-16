// SPDX-License-Identifier: AGPL-3.0-or-later
//! OAuth scope strings and client ids (`data-model.md` §10.3, §10.4; `threat-model.md` AUTH-1,
//! AUTH-2).
//!
//! - Every scope begins with `atproto`; `transition:generic` and `blob:*/*` are never requested.
//! - The default for the final namespace is `atproto include:NS.authFull` + the enumerated blob
//!   scopes; on `invalid_scope` at PAR time (before the browser opens) the enumerated
//!   `repo?collection=…` form is used. `.temp` builds (and so the devnet) use only the
//!   enumerated form, because `.temp` publishes no permission set.
//! - The loopback `client_id` is `http://localhost?redirect_uri=<pct>&scope=<pct(SUPERSET)>`:
//!   no port, an empty path, RFC 3986 percent-encoding.

use lessdugin_atp::xrpc::percent_encode;
use lessdugin_lexicon::ns::{NS, NS_FINAL};

pub use crate::auth::allowlist::COLLECTIONS;

/// The enumerated blob scopes (AUTH-2).
pub const BLOB_SCOPES: &str =
    "blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf";

/// The hosted native client metadata (`deploy/site/oauth/cli-client-metadata.json`).
pub const HOSTED_CLIENT_ID: &str =
    "https://lessdugin.7778777.online/oauth/cli-client-metadata.json";

/// True when this build writes the final (published) namespace.
pub fn is_final_ns() -> bool {
    NS == NS_FINAL
}

/// `repo?collection=NS.forum&collection=NS.membership&…` for this build's namespace.
pub fn repo_scope() -> String {
    let params: Vec<String> = COLLECTIONS
        .iter()
        .map(|c| format!("collection={NS}.{c}"))
        .collect();
    format!("repo?{}", params.join("&"))
}

/// The enumerated scope (the fallback, the devnet, `.temp` builds).
pub fn enumerated_scope() -> String {
    format!("atproto {} {BLOB_SCOPES}", repo_scope())
}

/// The permission-set scope (final namespace only).
pub fn include_scope() -> Option<String> {
    is_final_ns().then(|| format!("atproto include:{NS}.authFull {BLOB_SCOPES}"))
}

/// The scopes to try at PAR time, in order.
pub fn scope_candidates() -> Vec<String> {
    include_scope()
        .into_iter()
        .chain(std::iter::once(enumerated_scope()))
        .collect()
}

/// `SUPERSET` (`data-model.md` §10.4): every scope any variant may request.
pub fn superset_scope() -> String {
    if is_final_ns() {
        format!(
            "atproto include:{NS}.authFull {} {BLOB_SCOPES}",
            repo_scope()
        )
    } else {
        enumerated_scope()
    }
}

/// The loopback `client_id` for a redirect URI such as `http://127.0.0.1:53124/callback`.
pub fn loopback_client_id(redirect_uri: &str) -> String {
    format!(
        "http://localhost?redirect_uri={}&scope={}",
        percent_encode(redirect_uri),
        percent_encode(&superset_scope())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_follow_the_spec() {
        let e = enumerated_scope();
        assert!(e.starts_with("atproto repo?collection="));
        assert_eq!(e.matches("collection=").count(), 10);
        assert!(e.ends_with(BLOB_SCOPES));
        for s in scope_candidates().iter().chain([&superset_scope()]) {
            assert!(s.starts_with("atproto "), "{s}");
            assert!(!s.contains("transition:"), "{s}");
            assert!(!s.contains("blob:*/*"), "{s}");
        }
        assert_eq!(scope_candidates().last(), Some(&e));
        if is_final_ns() {
            assert_eq!(
                include_scope().unwrap(),
                format!("atproto include:online.7778777.lessdugin.authFull {BLOB_SCOPES}")
            );
        } else {
            assert_eq!(scope_candidates(), vec![e.clone()]);
            assert!(e.contains("collection=online.7778777.lessdugin.temp.wikiRevision&"));
            assert!(!superset_scope().contains("include:"));
        }
    }

    #[test]
    fn loopback_client_id_shape() {
        let id = loopback_client_id("http://127.0.0.1:53124/callback");
        assert!(
            id.starts_with(
                "http://localhost?redirect_uri=http%3A%2F%2F127.0.0.1%3A53124%2Fcallback&scope=atproto%20"
            ),
            "{id}"
        );
        // No port and an empty path on the client_id itself.
        let u = url::Url::parse(&id).unwrap();
        assert_eq!(u.port(), None);
        assert_eq!(u.host_str(), Some("localhost"));
        let pairs: Vec<(String, String)> = u.query_pairs().into_owned().collect();
        assert_eq!(pairs[0].1, "http://127.0.0.1:53124/callback");
        assert_eq!(pairs[1].1, superset_scope());
        assert!(!id.contains(' ') && !id.contains('*'));
    }
}
