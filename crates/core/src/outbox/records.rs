// SPDX-License-Identifier: AGPL-3.0-or-later
//! Record builders: lexicon-valid JSON values with their canonical record keys
//! (`data-model.md` §3, §6, §8.1).
//!
//! Values come from the [`lessdugin_lexicon::records`] structs (which always write `$type`, W1,
//! and keep unknown fields, W3); keys come from [`lessdugin_lexicon::keys`] (never re-derived
//! here). [`check`] runs the single-record validity rules (§7.2–§7.3) and the reserved-name rule
//! (W10) on what a write is about to persist.

use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_lexicon::collection::{Collection, nsid};
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::{
    Comment, Forum, Membership, ModAction, Opinion, Post, Profile, Revision, Sequence, StrongRef,
    WikiRevision,
};
use lessdugin_lexicon::values::RevisionOp;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::engine::EngineError;
use crate::outbox::model::Op;

/// A record ready to write: collection, canonical rkey and value.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// Collection NSID (this build's namespace).
    pub collection: &'static str,
    /// The canonical record key.
    pub rkey: String,
    /// The value (JSON).
    pub value: Value,
}

impl Built {
    /// The record's at-uri in `did`'s repo.
    pub fn uri(&self, did: &Did) -> String {
        format!("at://{did}/{}/{}", self.collection, self.rkey)
    }

    /// A create op of this record.
    pub fn create_op(&self) -> Op {
        Op::create(self.collection, &self.rkey, &self.value)
    }

    /// An update op to this value.
    pub fn update_op(&self) -> Op {
        Op::update(self.collection, &self.rkey, &self.value)
    }
}

fn key_err(e: keys::KeyError) -> EngineError {
    EngineError::invalid_input(format!("cannot derive the record key: {e}"))
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, EngineError> {
    serde_json::to_value(v).map_err(|e| EngineError::internal(format!("record JSON: {e}")))
}

/// Writer's clock as a datetime (`YYYY-MM-DDTHH:MM:SS.sssZ`, W5).
pub fn datetime_ms(ms: i64) -> Result<Datetime, EngineError> {
    Datetime::from_millis(ms).map_err(|e| EngineError::internal(format!("clock: {e}")))
}

/// Parses a canonical at-uri.
pub fn at_uri(s: &str) -> Result<AtUri, EngineError> {
    AtUri::parse(s).map_err(|e| EngineError::invalid_ref(format!("{s:?}: {e}")))
}

/// `membership` (rkey `fk(forum)`).
pub fn membership(
    forum: &AtUri,
    note: Option<&str>,
    now: &Datetime,
) -> Result<Built, EngineError> {
    let mut m = Membership::new(forum.clone(), now.clone());
    m.note = note.map(str::to_owned);
    Ok(Built {
        collection: nsid::MEMBERSHIP,
        rkey: keys::membership_rkey(forum.as_str()).map_err(key_err)?,
        value: to_value(&m)?,
    })
}

/// `post` (rkey `{fk}-{tid}`).
pub fn post(p: &Post, tid: &str) -> Result<Built, EngineError> {
    Ok(Built {
        collection: nsid::POST,
        rkey: keys::post_rkey(p.forum.as_str(), tid).map_err(key_err)?,
        value: to_value(p)?,
    })
}

/// `comment` (rkey `{fk}-{rootTid}-{tid}`).
pub fn comment(c: &Comment, tid: &str) -> Result<Built, EngineError> {
    Ok(Built {
        collection: nsid::COMMENT,
        rkey: keys::comment_rkey(NS, c.forum.as_str(), c.root.uri.as_str(), tid)
            .map_err(key_err)?,
        value: to_value(c)?,
    })
}

/// `forum` (rkey = the slug, which must be a SLUG).
pub fn forum(slug: &str, f: &Forum) -> Result<Built, EngineError> {
    if !keys::is_slug(slug) {
        return Err(EngineError::invalid_input(format!(
            "forum slug {slug:?} must match [a-z0-9]+(-[a-z0-9]+)* (at most 64 bytes)"
        )));
    }
    Ok(Built {
        collection: nsid::FORUM,
        rkey: slug.to_owned(),
        value: to_value(f)?,
    })
}

/// A `revision` snapshot of `record` (whose CID is `subject.cid`), rkey `P(subject)-{tid}`.
pub fn revision(
    subject: StrongRef,
    op: RevisionOp,
    record: Map<String, Value>,
    summary: Option<&str>,
    created_at: &Datetime,
    rkey: &str,
) -> Result<Built, EngineError> {
    let mut r = Revision::new(subject, op, record, created_at.clone());
    r.summary = summary.map(str::to_owned);
    Ok(Built {
        collection: nsid::REVISION,
        rkey: rkey.to_owned(),
        value: to_value(&r)?,
    })
}

/// The rkey of a revision of `subject` with `tid` (`P(subject) + "-" + tid`).
pub fn revision_rkey(subject: &str, tid: &str) -> Result<String, EngineError> {
    keys::revision_rkey(NS, subject, tid).map_err(key_err)
}

/// `opinion` (rkey `{fk}-{stid}-{h16("opinion", subject.uri)}`).
pub fn opinion(o: &Opinion) -> Result<Built, EngineError> {
    Ok(Built {
        collection: nsid::OPINION,
        rkey: opinion_rkey(o.forum.as_str(), o.subject.uri.as_str())?,
        value: to_value(o)?,
    })
}

/// The opinion rkey of `(forum, subject)`.
pub fn opinion_rkey(forum: &str, subject: &str) -> Result<String, EngineError> {
    keys::opinion_rkey(NS, forum, subject).map_err(key_err)
}

/// `modAction` (rkey `{fk}-{h16("modAction", scope_eff, subject, actionKey)}`).
pub fn mod_action(m: &ModAction) -> Result<Built, EngineError> {
    let value = to_value(m)?;
    let action = value
        .get("action")
        .and_then(Value::as_object)
        .ok_or_else(|| EngineError::invalid_input("modAction.action is not an object"))?;
    let rkey = keys::mod_action_rkey(
        NS,
        m.forum.as_str(),
        m.scope.as_deref(),
        &m.subject,
        action,
    )
    .map_err(key_err)?;
    Ok(Built {
        collection: nsid::MOD_ACTION,
        rkey,
        value,
    })
}

/// `wikiRevision` (rkey `{fk}-{tid}`).
pub fn wiki_revision(w: &WikiRevision, tid: &str) -> Result<Built, EngineError> {
    Ok(Built {
        collection: nsid::WIKI_REVISION,
        rkey: keys::wiki_revision_rkey(w.forum.as_str(), tid).map_err(key_err)?,
        value: to_value(w)?,
    })
}

/// `sequence` (rkey `{fk}-{tid}`).
pub fn sequence(s: &Sequence, tid: &str) -> Result<Built, EngineError> {
    Ok(Built {
        collection: nsid::SEQUENCE,
        rkey: keys::sequence_rkey(s.forum.as_str(), tid).map_err(key_err)?,
        value: to_value(s)?,
    })
}

/// `profile` (rkey `self`).
pub fn profile(p: &Profile) -> Result<Built, EngineError> {
    Ok(Built {
        collection: nsid::PROFILE,
        rkey: keys::PROFILE_RKEY.to_owned(),
        value: to_value(p)?,
    })
}

/// The CIDv1 (dag-cbor, sha2-256) of a record value's DAG-CBOR encoding.
pub fn record_cid(value: &Value) -> Result<String, EngineError> {
    lessdugin_atp::datamodel::record_cid(value)
        .map(|c| c.to_string())
        .map_err(|e| EngineError::invalid_input(format!("not an atproto record value: {e}")))
}

/// Refuses a value that fails the single-record validity rules (§7.2–§7.3) at
/// `at://author/collection/rkey`, or (for `new` records) carries a reserved field (W10).
pub fn check(
    author: &Did,
    collection: &str,
    rkey: &str,
    value: &Value,
    new: bool,
) -> Result<(), EngineError> {
    let uri = at_uri(&format!("at://{author}/{collection}/{rkey}"))?;
    let Some(coll) = Collection::from_nsid(collection) else {
        // Not one of ours (`app.bsky.actor.profile` is never written): the PDS decides.
        return Ok(());
    };
    let v = lessdugin_lexicon::validity::check(collection, &uri, value);
    if !v.valid {
        let rules: Vec<String> = v
            .violations
            .iter()
            .take(8)
            .map(ToString::to_string)
            .collect();
        return Err(EngineError::invalid_input(format!(
            "invalid {} record: {}",
            coll.short_name(),
            rules.join("; ")
        ))
        .with_detail(
            "rules",
            v.rule_ids()
                .iter()
                .map(|r| Value::String(r.as_str().to_owned()))
                .collect::<Vec<_>>(),
        ));
    }
    if new {
        let reserved = lessdugin_lexicon::reserved::reserved_fields_in(coll, value);
        if !reserved.is_empty() {
            return Err(EngineError::invalid_input(format!(
                "reserved field(s) {} are never written (W10)",
                reserved.join(", ")
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_lexicon::records::{Action, HideAction, Markdown, Md};
    use serde_json::json;

    const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

    fn now() -> Datetime {
        datetime_ms(1_760_000_000_000).unwrap()
    }

    fn forum_uri() -> AtUri {
        at_uri(&format!("at://{DID}/{}/rationality", nsid::FORUM)).unwrap()
    }

    #[test]
    fn builders_are_valid_with_canonical_keys() {
        let did = Did::parse(DID).unwrap();
        let f = forum_uri();
        let fk = keys::fk(f.as_str()).unwrap();
        let tid = "3l2s5xxv2ze2c";

        let m = membership(&f, Some("hi"), &now()).unwrap();
        assert_eq!(m.rkey, fk);
        check(&did, m.collection, &m.rkey, &m.value, true).unwrap();

        let p = post(&Post::article(f.clone(), "Title", "body", now()), tid).unwrap();
        assert_eq!(p.rkey, format!("{fk}-{tid}"));
        check(&did, p.collection, &p.rkey, &p.value, true).unwrap();
        assert_eq!(p.value["$type"], nsid::POST);

        let root = StrongRef::new(at_uri(&p.uri(&did)).unwrap(), record_cid(&p.value).unwrap());
        let c = comment(
            &Comment::new(f.clone(), root.clone(), None, "hey", now()),
            "3l2s6aaaaaaa2",
        )
        .unwrap();
        assert_eq!(c.rkey, format!("{fk}-{tid}-3l2s6aaaaaaa2"));
        check(&did, c.collection, &c.rkey, &c.value, true).unwrap();

        let rk = revision_rkey(root.uri.as_str(), "3l2s7aaaaaaa2").unwrap();
        assert_eq!(rk, format!("{fk}-{tid}-3l2s7aaaaaaa2"));
        let r = revision(
            root.clone(),
            RevisionOp::Update,
            p.value.as_object().unwrap().clone(),
            Some("typo"),
            &now(),
            &rk,
        )
        .unwrap();
        check(&did, r.collection, &r.rkey, &r.value, true).unwrap();

        let o = opinion(&Opinion::new(root.clone(), f.clone(), now())).unwrap();
        assert!(o.rkey.starts_with(&format!("{fk}-{tid}-")));
        check(&did, o.collection, &o.rkey, &o.value, true).unwrap();

        let mut ma = ModAction::new(f.clone(), root.uri.as_str(), Action::Hide(HideAction::hide()), now());
        ma.reason = Some("spam".into());
        let ma = mod_action(&ma).unwrap();
        check(&did, ma.collection, &ma.rkey, &ma.value, true).unwrap();

        let fo = forum("rationality", &Forum::create("R", now(), lessdugin_lexicon::values::MembershipMode::Open)).unwrap();
        check(&did, fo.collection, &fo.rkey, &fo.value, true).unwrap();
        assert!(forum("Bad Slug", &Forum::create("R", now(), lessdugin_lexicon::values::MembershipMode::Open)).is_err());

        let pr = profile(&Profile {
            bio: Some(Md::Known(Markdown::inline("me"))),
            ..Profile::default()
        })
        .unwrap();
        assert_eq!(pr.rkey, "self");
        check(&did, pr.collection, &pr.rkey, &pr.value, true).unwrap();
    }

    #[test]
    fn check_refuses_invalid() {
        let did = Did::parse(DID).unwrap();
        let f = forum_uri();
        let p = post(&Post::article(f, "Title", "body", now()), "3l2s5xxv2ze2c").unwrap();
        // a wrong key fails PO1/G5
        let e = check(&did, p.collection, "aaaaaaaaaaaa-3l2s5xxv2ze2c", &p.value, true).unwrap_err();
        assert_eq!(e.code, crate::engine::ErrorCode::InvalidInput);
        assert!(e.detail.unwrap()["rules"].to_string().contains("G5"));
        // a float is not in the data model
        let mut v = p.value.clone();
        v["n"] = json!(1.5);
        assert!(record_cid(&v).is_err());
    }
}
