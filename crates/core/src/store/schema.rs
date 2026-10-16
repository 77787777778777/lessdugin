// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `cache` key schema (`sync.md` §10.4) and the record scope rule.
//!
//! | Key | Value | Module |
//! |---|---|---|
//! | `("rec", scope, coll, did, rkey)` | [`super::records::RecordRow`] | [`super::records`] |
//! | `("tomb", scope, coll, did, rkey)` | [`super::records::TombRow`] | [`super::records`] |
//! | `("repo", did)` | [`super::repo::RepoRow`] | [`super::repo`] |
//! | `("host", origin)` | [`super::repo::HostRow`] | [`super::repo`] |
//! | `("blob", cid)` | [`super::blob::BlobRow`] | [`super::blob`] |
//! | `("cur", source)` | [`super::blob::CursorRow`] | [`super::blob`] |
//! | `("forum", fk)` | [`super::blob::ForumRow`] | [`super::blob`] |
//! | `("meta", "schema")` | integer | [`open_cache`] |
//! | `("id", "did" \| "handle", …)` | identity rows | [`crate::identity`] (owns its keys) |
//! | `("disc", fk, did)` | discovery rows | discovery |
//!
//! **Scope.** `scope = rkey[0..12]` if `coll` ∈ `RANGED ∪ {NS.membership}` and `rkey` matches
//! `^FK$` or `^FK-` (`FK = [2-7a-z]{12}`); otherwise [`GLOBAL_SCOPE`] (`"_"`). A forum is then one
//! prefix scan `("rec", fk)`, a repo's range of one collection one prefix scan
//! `("rec", fk, coll, did)`.

use lessdugin_lexicon::ns::NS;

use crate::host::{HostError, KeyRange, KvOp, KvStore};
use crate::store::key::{self, Part};

/// The scope of records that belong to no forum range (declarations, profiles, keys that do not
/// start with an fk).
pub const GLOBAL_SCOPE: &str = "_";

/// The cache schema version (`("meta", "schema")`). A mismatch drops the whole cache (I1).
pub const CACHE_SCHEMA_VERSION: i64 = 1;

/// Short names of the `RANGED` collections (`sync.md` §0.5): listed by `{fk}-` key range.
pub const RANGED_SHORT: [&str; 7] = [
    "post",
    "comment",
    "revision",
    "opinion",
    "modAction",
    "wikiRevision",
    "sequence",
];

/// Key prefix tags.
pub mod tag {
    /// Record versions.
    pub const REC: &str = "rec";
    /// Tombstones.
    pub const TOMB: &str = "tomb";
    /// Repo state.
    pub const REPO: &str = "repo";
    /// Host capabilities.
    pub const HOST: &str = "host";
    /// Blob metadata.
    pub const BLOB: &str = "blob";
    /// Live-source cursors.
    pub const CUR: &str = "cur";
    /// Forum access times.
    pub const FORUM: &str = "forum";
    /// Schema version.
    pub const META: &str = "meta";
}

/// The short name of `coll` if it is in this build's namespace (`NS.<short>`).
fn ns_short(coll: &str) -> Option<&str> {
    coll.strip_prefix(NS)?.strip_prefix('.')
}

/// `true` if `coll` is one of this build's `RANGED` collections.
pub fn is_ranged(coll: &str) -> bool {
    ns_short(coll).is_some_and(|s| RANGED_SHORT.contains(&s))
}

/// `true` if `coll` is this build's `NS.membership`.
pub fn is_membership(coll: &str) -> bool {
    ns_short(coll) == Some("membership")
}

/// The `RANGED` cap of `data-model.md` §7.4 for `coll`: 100,000 for `opinion`, 20,000 otherwise.
pub fn fetch_cap(coll: &str) -> u32 {
    if ns_short(coll) == Some("opinion") {
        crate::limits::FETCH_CAP_OPINION
    } else {
        crate::limits::FETCH_CAP_DEFAULT
    }
}

/// `true` if `s` is an fk token: `[2-7a-z]{12}`.
pub fn is_fk(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| matches!(b, b'2'..=b'7' | b'a'..=b'z'))
}

/// `fkOf(rkey)`: the first 12 characters of an rkey that starts with an fk token followed by the
/// end of the key or `-`; `None` otherwise.
pub fn fk_prefix(rkey: &str) -> Option<&str> {
    let head = rkey.get(..12)?;
    (is_fk(head) && matches!(rkey.as_bytes().get(12), None | Some(b'-'))).then_some(head)
}

/// The scope of a record (`sync.md` §10.4): its fk for keys `^FK$` / `^FK-` of `RANGED` and
/// membership records, else [`GLOBAL_SCOPE`].
pub fn scope_of<'a>(coll: &str, rkey: &'a str) -> &'a str {
    if (is_ranged(coll) || is_membership(coll))
        && let Some(fk) = fk_prefix(rkey)
    {
        return fk;
    }
    GLOBAL_SCOPE
}

/// `true` if `rkey` lies in the `{fk}-` range listing of `fk` (`sync.md` §5.3).
pub fn in_range(fk: &str, rkey: &str) -> bool {
    rkey.len() > fk.len() + 1 && rkey.starts_with(fk) && rkey.as_bytes()[fk.len()] == b'-'
}

/// `("rec", scope, coll, did, rkey)`.
pub fn rec_key(scope: &str, coll: &str, did: &str, rkey: &str) -> Vec<u8> {
    crate::key!(tag::REC, scope, coll, did, rkey)
}

/// `("tomb", scope, coll, did, rkey)`.
pub fn tomb_key(scope: &str, coll: &str, did: &str, rkey: &str) -> Vec<u8> {
    crate::key!(tag::TOMB, scope, coll, did, rkey)
}

/// The encoded prefix `(tag, parts…)`.
pub fn prefix(tag: &str, parts: &[&str]) -> Vec<u8> {
    let mut v = key::encode(&[Part::from(tag)]);
    for p in parts {
        key::encode_part(&mut v, &Part::from(*p));
    }
    v
}

/// `("repo", did)`.
pub fn repo_key(did: &str) -> Vec<u8> {
    crate::key!(tag::REPO, did)
}

/// `("host", origin)`.
pub fn host_key(origin: &str) -> Vec<u8> {
    crate::key!(tag::HOST, origin)
}

/// `("blob", cid)`.
pub fn blob_key(cid: &str) -> Vec<u8> {
    crate::key!(tag::BLOB, cid)
}

/// `("cur", source)`: `source` = `"jetstream"` or `"indexer:" + did + ":" + fk`.
pub fn cur_key(source: &str) -> Vec<u8> {
    crate::key!(tag::CUR, source)
}

/// `("forum", fk)`.
pub fn forum_key(fk: &str) -> Vec<u8> {
    crate::key!(tag::FORUM, fk)
}

/// `("meta", "schema")`.
pub fn meta_schema_key() -> Vec<u8> {
    crate::key!(tag::META, "schema")
}

/// The components of a record or tombstone key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecKey {
    /// The fk, or [`GLOBAL_SCOPE`].
    pub scope: String,
    /// The collection NSID.
    pub collection: String,
    /// The repo DID.
    pub did: String,
    /// The record key.
    pub rkey: String,
}

impl RecKey {
    /// The key of a record uri's parts (the scope is computed).
    pub fn new(collection: &str, did: &str, rkey: &str) -> Self {
        Self {
            scope: scope_of(collection, rkey).to_owned(),
            collection: collection.to_owned(),
            did: did.to_owned(),
            rkey: rkey.to_owned(),
        }
    }

    /// The parts of a canonical at-uri.
    pub fn of_uri(uri: &lessdugin_atp::syntax::AtUri) -> Self {
        Self::new(
            uri.collection().as_str(),
            uri.did().as_str(),
            uri.rkey().as_str(),
        )
    }

    /// `at://did/collection/rkey`.
    pub fn uri(&self) -> String {
        format!("at://{}/{}/{}", self.did, self.collection, self.rkey)
    }

    /// The `rec` key.
    pub fn rec(&self) -> Vec<u8> {
        rec_key(&self.scope, &self.collection, &self.did, &self.rkey)
    }

    /// The `tomb` key.
    pub fn tomb(&self) -> Vec<u8> {
        tomb_key(&self.scope, &self.collection, &self.did, &self.rkey)
    }

    /// Parses an encoded `rec` or `tomb` key (`expected_tag`); `None` for anything else.
    pub fn parse(bytes: &[u8], expected_tag: &str) -> Option<Self> {
        fn text(p: Part) -> Option<String> {
            match p {
                Part::Bytes(b) => String::from_utf8(b).ok(),
                _ => None,
            }
        }
        let parts = key::decode(bytes).ok()?;
        let [t, scope, coll, did, rkey] = <[Part; 5]>::try_from(parts).ok()?;
        if text(t)? != expected_tag {
            return None;
        }
        Some(Self {
            scope: text(scope)?,
            collection: text(coll)?,
            did: text(did)?,
            rkey: text(rkey)?,
        })
    }
}

/// Opens the cache: if `("meta", "schema")` is missing or differs from [`CACHE_SCHEMA_VERSION`],
/// every key is deleted (the cache is disposable, I1) and the current version is written.
/// Returns `true` if the cache was reset (a new, empty cache included).
pub async fn open_cache<K: KvStore>(kv: &K) -> Result<bool, HostError> {
    let key = meta_schema_key();
    let current = kv
        .get_many(vec![key.clone()])
        .await?
        .pop()
        .flatten()
        .and_then(|b| super::row::decode::<i64>(&b).ok());
    if current == Some(CACHE_SCHEMA_VERSION) {
        return Ok(false);
    }
    let value =
        super::row::encode(&CACHE_SCHEMA_VERSION).map_err(|e| HostError::Storage(e.to_string()))?;
    kv.write(vec![
        KvOp::DeleteRange(KeyRange::prefix(&[])),
        KvOp::Put(key, value),
    ])
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Dir;
    use crate::store::mem::MemKv;
    use futures::executor::block_on;

    fn c(short: &str) -> String {
        format!("{NS}.{short}")
    }

    const FK: &str = "uetjixxr3a33";

    #[test]
    fn fk_tokens() {
        assert!(is_fk(FK));
        assert!(!is_fk("uetjixxr3a3"));
        assert!(!is_fk("uetjixxr3a331"));
        assert!(!is_fk("UETJIXXR3A33"));
        assert!(!is_fk("uetjixxr3a31")); // '1' is not in the alphabet
        assert_eq!(fk_prefix(FK), Some(FK));
        assert_eq!(fk_prefix(&format!("{FK}-3l2s5xxv2ze2c")), Some(FK));
        assert_eq!(fk_prefix(&format!("{FK}.x")), None);
        assert_eq!(fk_prefix(&format!("{FK}x")), None);
        assert_eq!(fk_prefix("self"), None);
        assert_eq!(fk_prefix("é"), None);
    }

    #[test]
    fn scope_rule() {
        let post = format!("{FK}-3l2s5xxv2ze2c");
        assert_eq!(scope_of(&c("post"), &post), FK);
        assert_eq!(scope_of(&c("membership"), FK), FK);
        assert_eq!(
            scope_of(&c("opinion"), &format!("{FK}-~-wcz54x7liy3jsh3g")),
            FK
        );
        // a ranged collection with a key that does not start with an fk
        assert_eq!(scope_of(&c("post"), "3l2s5xxv2ze2c"), GLOBAL_SCOPE);
        // global collections, whatever the key
        assert_eq!(scope_of(&c("forum"), FK), GLOBAL_SCOPE);
        assert_eq!(scope_of(&c("profile"), "self"), GLOBAL_SCOPE);
        assert_eq!(scope_of("app.bsky.actor.profile", "self"), GLOBAL_SCOPE);
        // another namespace is not ours
        let other = if NS == lessdugin_lexicon::ns::NS_TEMP {
            lessdugin_lexicon::ns::NS_FINAL
        } else {
            lessdugin_lexicon::ns::NS_TEMP
        };
        assert_eq!(scope_of(&format!("{other}.post"), &post), GLOBAL_SCOPE);
        assert!(is_ranged(&c("wikiRevision")));
        assert!(!is_ranged(&c("membership")));
        assert!(!is_ranged(&format!("{NS}post")));
        assert_eq!(fetch_cap(&c("opinion")), 100_000);
        assert_eq!(fetch_cap(&c("post")), 20_000);
    }

    #[test]
    fn range_membership() {
        assert!(in_range(FK, &format!("{FK}-a")));
        assert!(!in_range(FK, FK));
        assert!(!in_range(FK, &format!("{FK}-")));
        assert!(!in_range(FK, &format!("{FK}.a")));
        assert!(!in_range(FK, "zzz"));
    }

    #[test]
    fn rec_keys_roundtrip_and_prefixes() {
        let did = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
        let k = RecKey::new(&c("post"), did, &format!("{FK}-3l2s5xxv2ze2c"));
        assert_eq!(k.scope, FK);
        assert_eq!(RecKey::parse(&k.rec(), tag::REC), Some(k.clone()));
        assert_eq!(RecKey::parse(&k.tomb(), tag::TOMB), Some(k.clone()));
        assert_eq!(RecKey::parse(&k.rec(), tag::TOMB), None);
        assert_eq!(RecKey::parse(&repo_key(did), tag::REC), None);
        assert!(k.rec().starts_with(&prefix(tag::REC, &[FK])));
        assert!(
            k.rec()
                .starts_with(&prefix(tag::REC, &[FK, &c("post"), did]))
        );
        assert!(!k.rec().starts_with(&prefix(tag::REC, &[FK, &c("comment")])));
        assert_eq!(
            k.uri(),
            format!("at://{did}/{}/{FK}-3l2s5xxv2ze2c", c("post"))
        );
    }

    #[test]
    fn open_cache_drops_on_mismatch() {
        let kv = MemKv::new();
        block_on(async {
            // fresh cache: nothing to drop but the version is written
            assert!(open_cache(&kv).await.unwrap());
            assert!(!open_cache(&kv).await.unwrap());
            kv.write(vec![KvOp::Put(repo_key("did:x"), vec![1])])
                .await
                .unwrap();
            assert!(!open_cache(&kv).await.unwrap());
            assert_eq!(kv.len(), 2);
            // another version: everything goes
            kv.write(vec![KvOp::Put(
                meta_schema_key(),
                super::super::row::encode(&(CACHE_SCHEMA_VERSION + 1)).unwrap(),
            )])
            .await
            .unwrap();
            assert!(open_cache(&kv).await.unwrap());
            let all = kv
                .scan(KeyRange::prefix(b""), Dir::Forward, 100)
                .await
                .unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].0, meta_schema_key());
        });
    }
}
