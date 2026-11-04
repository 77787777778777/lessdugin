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

/// The hosted native client metadata (`deploy/site/oauth/cli-client-metadata.json`): the
/// default; `config.toml` `oauth_client_id` or `login --oauth-client-id` names a preview or
/// self-hosted origin's (decision O-6).
pub const HOSTED_CLIENT_ID: &str =
    "https://lessdugin.7778777.online/oauth/cli-client-metadata.json";

/// True when this build writes the final (published) namespace.
pub fn is_final_ns() -> bool {
    NS == NS_FINAL
}

/// `repo?collection=NS.forum&collection=NS.membership&…` for this build's namespace.
pub fn repo_scope() -> String {
    repo_scope_in(NS)
}

fn repo_scope_in(ns: &str) -> String {
    let params: Vec<String> = COLLECTIONS
        .iter()
        .map(|c| format!("collection={ns}.{c}"))
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
    superset_in(NS, is_final_ns())
}

/// `SUPERSET` of namespace `ns`: `include:` only where a permission set is published (the
/// final namespace), then the enumerated `repo?collection=…`, then the blob scopes.
fn superset_in(ns: &str, published: bool) -> String {
    let include = if published {
        format!(" include:{ns}.authFull")
    } else {
        String::new()
    };
    format!("atproto{include} {} {BLOB_SCOPES}", repo_scope_in(ns))
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

    /// `data-model.md` §10.4 gives both metadata documents exactly: the shipped files are
    /// those documents, their scope is the final namespace's `SUPERSET` as this code builds it,
    /// and the hosted native client id is the shipped document's.
    #[test]
    fn shipped_metadata_is_the_spec_s() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let spec = std::fs::read_to_string(format!("{root}/docs/spec/data-model.md")).unwrap();
        let section = spec
            .split("### 10.4 Client metadata")
            .nth(1)
            .and_then(|s| s.split("\n### ").next())
            .expect("data-model.md §10.4");
        // the fenced blocks of the section: SUPERSET, the web document, the native document
        let blocks: Vec<&str> = section
            .split("```")
            .skip(1)
            .step_by(2)
            .map(|b| b.split_once('\n').map_or("", |(_, body)| body).trim())
            .collect();
        let [superset, web, native] = blocks[..] else {
            panic!("§10.4 has {} fenced blocks", blocks.len());
        };
        let final_superset = superset_in(NS_FINAL, true);
        assert_eq!(superset, final_superset);
        // a `.temp` build has no `include:` token (§10.4)
        let temp = superset_in(lessdugin_lexicon::ns::NS_TEMP, false);
        assert!(temp.starts_with("atproto repo?collection="), "{temp}");
        assert!(!temp.contains("include:"), "{temp}");
        if !is_final_ns() {
            assert_eq!(superset_scope(), temp);
        }
        for (block, file) in [
            (web, "client-metadata.json"),
            (native, "cli-client-metadata.json"),
        ] {
            let want: serde_json::Value = serde_json::from_str(block).unwrap();
            let path = format!("{root}/deploy/site/oauth/{file}");
            let shipped: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(shipped, want, "{file} differs from data-model.md §10.4");
            assert_eq!(shipped["scope"], final_superset.as_str(), "{file}");
        }
        let native: serde_json::Value = serde_json::from_str(native).unwrap();
        assert_eq!(native["client_id"], HOSTED_CLIENT_ID);
        assert_eq!(native["client_name"], "lessdugin CLI");
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
