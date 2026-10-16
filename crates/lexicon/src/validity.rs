// SPDX-License-Identifier: MIT OR Apache-2.0
//! Single-record validity (`data-model.md` §7.2–§7.3): [`check`] decides every rule that is a
//! pure function of a record's at-uri and value.
//!
//! An invalid record is excluded from the fold and appears only in the raw view, annotated with
//! its first failing rule ([`Validity::first_rule`]: rules are ordered as in the spec's tables,
//! G1–G10 first). Every rule that fails is reported, so one defect can fail several rules: a
//! wrong post key fails PO1 and the generic G5 (and G9 when the fk segment is wrong); a
//! `textBlob` in a comment body fails CO4 and G7; a disallowed reference collection fails both
//! G4 and the collection's own rule (CO2, OP1, RV2, MA4, …).
//!
//! **Valid but without effect.** Some valid records have no effect in v1. They are reported in
//! [`Validity::no_effect`], never as violations: the reserved modAction key form `{fk}-{tid}`
//! (MA2), unknown modAction scope, action `$type`, `#access.level`, `#tier.tier` or `#pin.scope`
//! (MA7), and `#lock` on a forum uri (MA9).
//!
//! # Rule coverage
//!
//! | Rule | Where | Notes |
//! |---|---|---|
//! | G1 | here | `$type` = collection NSID of this build; the collection is one of this build's (another prefix fails) |
//! | G2 | here | lexicon validation of the embedded schema ([`crate::validate`]) plus the atproto data model (no floats, well-formed `$link`/`$bytes`/blobs, `$type` strings) |
//! | G3 | here | legacy `{cid, mimeType}` blobs at schema blob positions |
//! | G4 | here | canonical at-uri, allowed collection (§2.3 table; `modAction.subject` per action), NS rkey grammar, in every reference field and array entry, including `markdown.links` of known `md` members |
//! | G5 | here | reported together with the collection's key rule (FO1, ME1, PO1, CO1, CO2, RV1, RV3, OP2, MA2, WR1, SQ1, PF1) |
//! | G6 | here | DAG-CBOR size ≤ 1,000,000 bytes |
//! | G7 | here | every `NS.markdown` member: exactly one of `text`/`textBlob`; `textBlob` only in `forum.about`, `post.body`, `wikiRevision.body`, `sequence.body` |
//! | G8 | here | `forum.featuredTags[]`, `post.tags[]`, `wikiRevision.slug`, `.parentTags[]`, `#react.react`, `#tagVote.tag` |
//! | G9 | here | fk segment of the key = `fk(forum)` (collections with a `forum` field) |
//! | G10 | here | nesting depth ≤ 32 (`$link`/`$bytes` objects are leaves) |
//! | FO1 | here | rkey is a SLUG |
//! | FO2, FO3 | entry | [`entry::moderators`], [`entry::usable_indexers`] |
//! | FO4 | G2 + entry | out-of-range integers fail G2; unknown kv values: [`crate::values`] `effective` |
//! | ME1 | here | |
//! | PO1, PO3 | here | |
//! | PO2 | definition | [`crate::values::PostKind::effective`], used by PO3–PO5 |
//! | PO4, PO5, PO6 | here + entry | the requirement here; ignored fields: [`entry::post_title`], [`entry::post_link`], [`entry::post_excerpt`] |
//! | PO7, PO8 | entry | [`entry::post_tags`], [`entry::self_labels`] |
//! | CO1–CO4 | here | |
//! | CO5 | **core fold** *(x)* | root post's `forum` = comment `forum` needs the root post |
//! | CO6 | **core fold** | `moderatorHat` needs the forum's current authorities |
//! | RV1–RV5 | here | RV5 recomputes the snapshot's CIDv1 (dag-cbor, sha256) |
//! | OP1, OP2 | here | |
//! | OP3 | entry + **core fold** | unknown axis/stance/value: [`crate::records::Opinion::karma`], [`entry::opinion_reacts`], [`entry::opinion_tag_votes`]; the palette check needs the forum policy |
//! | OP4 | entry | [`entry::opinion_tag_votes`] |
//! | OP5 | **core fold** | whether an opinion counts (`social.md` §5) |
//! | MA1–MA5 | here | MA4 also checks the `#pin` subject per pin scope; positive = keep, tier ≥ frontpage, pinned |
//! | MA6 | here + **core fold** | the action/value allowed in author scope here; "own thread or post" and the author gates in the fold |
//! | MA7 | here (no effect) | |
//! | MA8 | entry | [`entry::restrict_capabilities`] |
//! | MA9 | here (no effect) + entry | forum-uri lock; [`entry::lock_allowed`] |
//! | MA10 | here + **core fold** | `NS.wikiRevision` with the forum's fk here; "a revision of the same slug" in the fold |
//! | MA11, MA12 | here | |
//! | WR1 | here | |
//! | WR2 | here + entry | collection and fk here; duplicates: [`entry::wiki_parents`] |
//! | SQ1 | here | |
//! | SQ2, SQ3 | here + entry | [`entry::chapter_posts`], [`entry::sequence_cover`] |
//! | PF1, PF2 | here | |
//!
//! # Left to the core fold (cross-record)
//!
//! These need records other than the one checked, or the forum's current state, so [`check`]
//! never reports them:
//! - **CO5** *(x)*: the root post's `forum` equals the comment's (needs the root post);
//! - **CO6**: `moderatorHat` is honoured only for a current authority (needs the declaration);
//! - **OP3** (palette part): a react id outside the forum's palette (needs `policy.reactPalette`);
//! - **OP5**: whether an opinion counts (`social.md` §5: subject present and valid in the same
//!   forum, voter not the author, not banned, nullified or restricted, subject not spam);
//! - **MA6** (fold part): the subject is in the actor's own thread or post; author gates;
//! - **MA10** (fold part): `#tag.head` names a revision of the same slug;
//! - placement of comments whose `parent` is missing or in another thread (`social.md` threads;
//!   [`entry::comment_parent_in_thread`] is the structural part);
//! - admission (§7.1, `sync.md`), the fetch caps (§7.4; constants in [`crate::limits`]) and
//!   tie-breaks (§7.5).
//!
//! Records with a reserved `{fk}-{tid}` modAction key are checked for G1–G10 and MA10–MA12 only
//! (they have no effect).

pub mod entry;

use lessdugin_atp::datamodel::{Limits, json_to_ipld_with};
use lessdugin_atp::syntax::{AtUri, Did};
use serde_json::{Map, Value};

use crate::collection::{Collection, nsid, type_matches};
use crate::keys::{self, KeyError, ModActionKey, RkeyInput};
use crate::limits::{NESTING_MAX_RECORD, RECORD_CBOR_MAX_BYTES};
use crate::ns::NS;
use crate::schemas::registry;
use crate::validate::{Code, Options, Validator, nesting_depth};
use crate::values::{AccessLevel, ActionKind, PinScope, PostKind, RevisionOp, Scope, Tier};

macro_rules! rules {
    ($($id:ident => $how:ident, $text:literal;)*) => {
        /// A validity rule ID of `data-model.md` §7.2–§7.3, in spec order.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum RuleId {
            $(
                #[doc = $text]
                $id,
            )*
        }

        impl RuleId {
            /// Every rule, in spec order.
            pub const ALL: &'static [RuleId] = &[$(RuleId::$id),*];

            /// The rule ID as the spec writes it (`"G1"`, `"PO3"`, …).
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(RuleId::$id => stringify!($id),)*
                }
            }

            /// Parses a rule ID.
            pub fn parse(s: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|r| r.as_str() == s)
            }

            /// Where the rule is evaluated.
            pub const fn evaluation(self) -> Evaluation {
                match self {
                    $(RuleId::$id => Evaluation::$how,)*
                }
            }

            /// The rule's text (abridged from the spec).
            pub const fn text(self) -> &'static str {
                match self {
                    $(RuleId::$id => $text,)*
                }
            }
        }
    };
}

/// Where a rule is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Evaluation {
    /// Fully by [`check`].
    Here,
    /// Partly by [`check`] (or a no-effect annotation), partly at entry level ([`entry`]) or in
    /// the core fold (see the module table).
    Partly,
    /// Entry level only: never invalidates; helpers in [`entry`].
    Entry,
    /// Needs other records: the core fold.
    Fold,
    /// A definition used by other rules.
    Definition,
}

rules! {
    G1 => Here, "`value.$type` equals the record's collection NSID (after `NS` prefix mapping).";
    G2 => Here, "The value passes lexicon validation (required fields, JSON types, formats, lengths, graphemes, integer ranges, array lengths, blob accept and maxSize).";
    G3 => Here, "Blobs use the typed form; the legacy untyped form is invalid.";
    G4 => Here, "Every reference field holds a canonical at-uri in an allowed collection, matching the NS rkey grammar.";
    G5 => Here, "The rkey is canonical (§3.2), including every listed equality.";
    G6 => Here, "The record's DAG-CBOR encoding is at most 1,000,000 bytes.";
    G7 => Here, "Every `NS.markdown` value satisfies body rules 1 and 2 (§5.2).";
    G8 => Here, "Every slug-typed value matches `^[a-z0-9]+(-[a-z0-9]+)*$`.";
    G9 => Here, "Forum-scoped collections: `fk(value.forum)` equals the key's fk segment.";
    G10 => Here, "The value's nesting depth is at most 32.";
    FO1 => Here, "forum: the rkey matches SLUG.";
    FO2 => Entry, "forum (entry): moderators entries of the owner, with an unknown role, or for a DID already listed are ignored.";
    FO3 => Entry, "forum (entry): discovery.indexers entries are ignored unless https: with a hostname and no explicit port.";
    FO4 => Partly, "forum (entry): unknown policy kv values take their default; out-of-range integers fail G2.";
    ME1 => Here, "membership: `rkey = fk(forum)`.";
    PO1 => Here, "post: the rkey is `{fk}-{tid}` with `fk = fk(forum)`.";
    PO2 => Definition, "post: the effective kind is `kind` if known, else `article`.";
    PO3 => Here, "post: unless quickTake, `title` is present and contains a non-White_Space character.";
    PO4 => Partly, "post: quickTake requires `body`; its `title` is ignored.";
    PO5 => Partly, "post: kind link requires `link`; other kinds ignore it.";
    PO6 => Partly, "post: a `textBlob` body requires `excerpt`; otherwise `excerpt` is ignored.";
    PO7 => Entry, "post (entry): `tags` has set semantics.";
    PO8 => Entry, "post (entry): only the selfLabels member of `labels` is read.";
    CO1 => Here, "comment: the rkey is `{fk}-{rootTid}-{tid}` with `fk = fk(forum)`.";
    CO2 => Here, "comment: `root.uri` is an `NS.post` uri with rkey `{fk}-{T}`, the same fk, and `rootTid = T`.";
    CO3 => Here, "comment: `parent`, if present, is an `NS.comment` uri.";
    CO4 => Here, "comment: `body` is inline.";
    CO5 => Fold, "comment (x): if the root post is in the fold input, its `forum` equals the comment's.";
    CO6 => Fold, "comment (entry): `moderatorHat` is honoured only for a current authority.";
    RV1 => Here, "revision: `did(subject.uri)` is the repo DID.";
    RV2 => Here, "revision: `collection(subject.uri)` is post, comment, forum, sequence or modAction.";
    RV3 => Here, "revision: `rkey = P(subject) + \"-\" + TID`.";
    RV4 => Here, "revision: `op` is known; `delete` only for a modAction subject.";
    RV5 => Here, "revision: `record`, if present, is an object of the subject's collection whose CID equals `subject.cid`.";
    OP1 => Here, "opinion: `collection(subject.uri)` is post, comment, wikiRevision or sequence.";
    OP2 => Here, "opinion: `rkey = fk(forum)-stid(subject)-h16(\"opinion\", subject.uri)` and the subject's fk is `fk(forum)`.";
    OP3 => Partly, "opinion (entry): unknown axis values, stances and tag values, and reacts outside the palette, are discarded.";
    OP4 => Entry, "opinion (entry): tags only for post subjects; equal duplicates collapse, conflicting ones are all ignored.";
    OP5 => Fold, "opinion: whether an opinion counts is decided by the fold (social.md §5).";
    MA1 => Here, "modAction: `subject` is a did, a canonical at-uri or `tag:` + SLUG.";
    MA2 => Here, "modAction: the rkey is `^FK-H16$` and equals the derived key (`{fk}-{tid}` is reserved: valid, no effect).";
    MA3 => Here, "modAction: an at-uri subject has `fkOf(rkey(subject)) = fk(forum)`.";
    MA4 => Here, "modAction: the subject form and collection are allowed for the action.";
    MA5 => Here, "modAction: positive assertions carry `subjectCid`.";
    MA6 => Partly, "modAction: in author scope, the action is allowed in author scope.";
    MA7 => Partly, "modAction: unknown scope, action, access level, tier or pin scope: valid, no effect.";
    MA8 => Entry, "modAction (entry): unknown or duplicate restrict capabilities are ignored.";
    MA9 => Partly, "modAction: a `#lock` on a forum uri is reserved (no effect); allowed entries outside the thread are ignored.";
    MA10 => Partly, "modAction: `#tag.head` is an `NS.wikiRevision` uri with the forum's fk.";
    MA11 => Here, "modAction: `adopts`, if present, is an `NS.modAction` uri.";
    MA12 => Here, "modAction: `#tier.notice` is inline.";
    WR1 => Here, "wikiRevision: the rkey is `{fk}-{tid}` with `fk = fk(forum)`.";
    WR2 => Partly, "wikiRevision: every `parents[].uri` is an `NS.wikiRevision` uri with the forum's fk; duplicates collapse.";
    SQ1 => Here, "sequence: the rkey is `{fk}-{tid}` with `fk = fk(forum)`.";
    SQ2 => Partly, "sequence: every chapter entry is an `NS.post` uri; entries of other forums are not displayed.";
    SQ3 => Partly, "sequence: `#chapter.body` is inline; a non-image cover is ignored.";
    PF1 => Here, "profile: the rkey is `self`.";
    PF2 => Here, "profile: `bio` is inline.";
}

impl core::fmt::Display for RuleId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One failed rule (or, in [`Validity::no_effect`], one no-effect annotation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The rule.
    pub rule_id: RuleId,
    /// JSON path in the record value (`$` for the record or its key).
    pub path: String,
    /// What is wrong.
    pub message: String,
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} {}: {}", self.rule_id, self.path, self.message)
    }
}

/// The validity of one record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Validity {
    /// `true` if no rule failed.
    pub valid: bool,
    /// Every failed rule, in rule order (then discovery order).
    pub violations: Vec<Violation>,
    /// Why a valid record has no effect in v1 (MA2 reserved key, MA7, MA9).
    pub no_effect: Vec<Violation>,
}

impl Validity {
    /// The first failing rule (the raw-view annotation).
    pub fn first_rule(&self) -> Option<RuleId> {
        self.violations.first().map(|v| v.rule_id)
    }

    /// `true` if `rule` failed.
    pub fn fails(&self, rule: RuleId) -> bool {
        self.violations.iter().any(|v| v.rule_id == rule)
    }

    /// `true` if the record is valid and has an effect.
    pub fn is_effective(&self) -> bool {
        self.valid && self.no_effect.is_empty()
    }

    /// The failed rule IDs, deduplicated, in rule order.
    pub fn rule_ids(&self) -> Vec<RuleId> {
        let mut ids: Vec<RuleId> = self.violations.iter().map(|v| v.rule_id).collect();
        ids.dedup();
        ids
    }
}

/// Checks the validity of the record `value` stored at `uri` in the collection `collection_nsid`
/// (a build-namespace NSID; normally `uri.collection()`). See the module docs for the rules.
/// Never panics; work is linear in the size of `value`.
pub fn check(collection_nsid: &str, uri: &AtUri, value: &Value) -> Validity {
    let mut c = Checker {
        uri,
        rkey: uri.rkey().as_str(),
        obj: None,
        out: Vec::new(),
        no_effect: Vec::new(),
        key_failed: false,
    };
    c.run(collection_nsid, value);
    c.finish()
}

/// [`check`] for the collection of `uri`.
pub fn check_uri(uri: &AtUri, value: &Value) -> Validity {
    check(uri.collection().as_str(), uri, value)
}

/// Depth up to which the data-model conversion (G2) and the DAG-CBOR size (G6) are computed.
/// Deeper values already fail G10; this only bounds recursion.
const DATA_MODEL_DEPTH_CAP: usize = 64;

/// The subject forms of `modAction.subject` (MA1).
#[derive(Debug, Clone, PartialEq)]
enum SubjectForm {
    Did,
    AtUri(AtUri),
    Tag,
    Invalid,
}

struct Checker<'a> {
    uri: &'a AtUri,
    rkey: &'a str,
    obj: Option<&'a Map<String, Value>>,
    out: Vec<Violation>,
    no_effect: Vec<Violation>,
    key_failed: bool,
}

fn s(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

/// The collection of an at-uri string, split leniently (`at://A/C/K`).
fn coll_of(uri: &str) -> Option<Collection> {
    keys::split_uri(uri)
        .ok()
        .and_then(|(_, c, _)| Collection::from_nsid(c))
}

fn rkey_of(uri: &str) -> Option<&str> {
    keys::split_uri(uri).ok().map(|(_, _, k)| k)
}

fn names(cs: &[Collection]) -> String {
    cs.iter()
        .map(|c| c.short_name())
        .collect::<Vec<_>>()
        .join(", ")
}

impl<'a> Checker<'a> {
    fn v(&mut self, rule: RuleId, path: impl Into<String>, message: impl Into<String>) {
        self.out.push(Violation {
            rule_id: rule,
            path: path.into(),
            message: message.into(),
        });
    }

    fn note(&mut self, rule: RuleId, path: impl Into<String>, message: impl Into<String>) {
        self.no_effect.push(Violation {
            rule_id: rule,
            path: path.into(),
            message: message.into(),
        });
    }

    /// A key equality of §3.2 failed: the collection's rule, plus G5 once.
    fn key_fail(&mut self, rule: RuleId, message: impl Into<String>) {
        let message = message.into();
        if !self.key_failed {
            self.key_failed = true;
            self.v(
                RuleId::G5,
                "$",
                format!("rkey {:?} is not canonical ({rule}: {message})", self.rkey),
            );
        }
        self.v(rule, "$", message);
    }

    fn finish(mut self) -> Validity {
        self.out.sort_by_key(|v| v.rule_id);
        self.no_effect.sort_by_key(|v| v.rule_id);
        Validity {
            valid: self.out.is_empty(),
            violations: self.out,
            no_effect: self.no_effect,
        }
    }

    fn get(&self, k: &str) -> Option<&'a Value> {
        self.obj.and_then(|o| o.get(k))
    }

    fn str_field(&self, k: &str) -> Option<&'a str> {
        s(self.get(k))
    }

    fn run(&mut self, collection_nsid: &str, value: &'a Value) {
        let Some(coll) = Collection::from_nsid(collection_nsid) else {
            self.v(
                RuleId::G1,
                "$",
                format!("{collection_nsid:?} is not a record collection of this build ({NS})"),
            );
            return;
        };
        if self.uri.collection().as_str() != collection_nsid {
            self.v(
                RuleId::G1,
                "$",
                format!(
                    "the uri's collection {} is not {collection_nsid}",
                    self.uri.collection()
                ),
            );
        }
        let Some(obj) = value.as_object() else {
            self.v(RuleId::G2, "$", "a record must be a JSON object");
            return;
        };
        self.obj = Some(obj);
        match obj.get("$type") {
            Some(Value::String(t)) if t == collection_nsid => {}
            Some(Value::String(t)) => self.v(
                RuleId::G1,
                "$.$type",
                format!("$type {t:?} is not {collection_nsid}"),
            ),
            Some(_) => self.v(RuleId::G1, "$.$type", "$type must be a string"),
            None => self.v(RuleId::G1, "$.$type", "missing $type"),
        }

        let depth = self.generic(collection_nsid, value);
        if depth > DATA_MODEL_DEPTH_CAP {
            return; // too deep to walk safely; G10 already fails
        }

        if coll.has_forum_field() {
            self.forum_ref();
            self.g9();
        }
        match coll {
            Collection::Forum => self.forum(),
            Collection::Membership => self.membership(),
            Collection::Post => self.post(),
            Collection::Comment => self.comment(),
            Collection::Revision => self.revision(),
            Collection::Opinion => self.opinion(),
            Collection::ModAction => self.mod_action(),
            Collection::WikiRevision => self.wiki_revision(),
            Collection::Sequence => self.sequence(),
            Collection::Profile => self.profile(),
        }
    }

    /// G2, G3, G6, G10. Returns the nesting depth (capped).
    fn generic(&mut self, collection_nsid: &str, value: &Value) -> usize {
        let (depth, at) = nesting_depth(value, DATA_MODEL_DEPTH_CAP);
        if depth > NESTING_MAX_RECORD {
            let (_, first) = nesting_depth(value, NESTING_MAX_RECORD);
            self.v(
                RuleId::G10,
                first.or(at).unwrap_or_else(|| "$".into()),
                format!("nesting deeper than {NESTING_MAX_RECORD} containers"),
            );
        }
        let opts = Options {
            allow_legacy_blobs: false,
            max_depth: NESTING_MAX_RECORD,
            ..Options::default()
        };
        let report = Validator::new(registry(), opts).record(collection_nsid, None, value);
        for v in report.violations {
            let rule = match v.code {
                Code::RecordType | Code::Depth => continue, // G1 and G10 are checked above
                Code::LegacyBlob => RuleId::G3,
                _ => RuleId::G2,
            };
            self.v(rule, v.path, v.message);
        }
        if report.truncated {
            self.v(
                RuleId::G2,
                "$",
                "too many lexicon violations; report truncated",
            );
        }
        if depth <= DATA_MODEL_DEPTH_CAP {
            let limits = Limits::uncapped(DATA_MODEL_DEPTH_CAP);
            match json_to_ipld_with(value, &limits) {
                Err(e) => self.v(
                    RuleId::G2,
                    "$",
                    format!("not valid atproto data model: {e}"),
                ),
                Ok(ipld) => match lessdugin_atp::dagcbor::encode_with(&ipld, &limits) {
                    Ok(bytes) if bytes.len() > RECORD_CBOR_MAX_BYTES => self.v(
                        RuleId::G6,
                        "$",
                        format!(
                            "DAG-CBOR encoding is {} bytes (max {RECORD_CBOR_MAX_BYTES})",
                            bytes.len()
                        ),
                    ),
                    Ok(_) => {}
                    Err(e) => self.v(RuleId::G2, "$", format!("not encodable as DAG-CBOR: {e}")),
                },
            }
        }
        depth
    }

    // -----------------------------------------------------------------------------------------
    // Generic field helpers

    /// G4 for one reference string. `allowed = None` allows any collection. Returns the parsed
    /// uri when it is canonical.
    fn reference(
        &mut self,
        path: &str,
        uri: &str,
        allowed: Option<&[Collection]>,
    ) -> Option<AtUri> {
        let u = match AtUri::parse(uri) {
            Ok(u) => u,
            Err(e) => {
                self.v(RuleId::G4, path, format!("not a canonical at-uri: {e}"));
                return None;
            }
        };
        let coll = Collection::from_nsid(u.collection().as_str());
        if let Some(allowed) = allowed
            && !coll.is_some_and(|c| allowed.contains(&c))
        {
            let msg = if allowed.is_empty() {
                format!("an at-uri ({}) is not allowed here", u.collection())
            } else {
                format!(
                    "collection {} is not allowed here (allowed: {})",
                    u.collection(),
                    names(allowed)
                )
            };
            self.v(RuleId::G4, path, msg);
        }
        if let Some(c) = coll
            && !keys::matches_grammar(c, u.rkey().as_str())
        {
            self.v(
                RuleId::G4,
                path,
                format!(
                    "rkey {:?} does not match the {c} key grammar",
                    u.rkey().as_str()
                ),
            );
        }
        Some(u)
    }

    /// G4 for a strong reference object's `uri`; returns the uri string.
    fn sref(
        &mut self,
        path: &str,
        v: Option<&'a Value>,
        allowed: &[Collection],
    ) -> Option<&'a str> {
        let uri = s(v?.get("uri"))?;
        self.reference(&format!("{path}.uri"), uri, Some(allowed));
        Some(uri)
    }

    /// G7 (and the field's inline rule) plus G4 of `links` for one `md` value.
    fn md(
        &mut self,
        path: &str,
        v: Option<&'a Value>,
        spillable: bool,
        inline_rule: Option<RuleId>,
    ) {
        let Some(m) = v.and_then(Value::as_object) else {
            return;
        };
        match s(m.get("$type")) {
            Some(t) if type_matches(t, nsid::MARKDOWN) => {}
            _ => return, // an unknown member is left alone (§5.2 rule 6)
        }
        let (text, blob) = (m.contains_key("text"), m.contains_key("textBlob"));
        if text && blob {
            self.v(RuleId::G7, path, "both text and textBlob are present");
        } else if !text && !blob {
            self.v(RuleId::G7, path, "neither text nor textBlob is present");
        }
        if blob && !spillable {
            let p = format!("{path}.textBlob");
            self.v(RuleId::G7, &p, "this field is inline only (no textBlob)");
            if let Some(r) = inline_rule {
                self.v(r, &p, "this field is inline only (no textBlob)");
            }
        }
        if let Some(Value::Array(links)) = m.get("links") {
            for (i, l) in links.iter().enumerate() {
                if let Some(uri) = l.as_str() {
                    self.reference(&format!("{path}.links[{i}]"), uri, None);
                }
            }
        }
    }

    /// `true` if the `md` value is a known member that uses `textBlob`.
    fn md_spilled(v: Option<&Value>) -> bool {
        v.and_then(Value::as_object).is_some_and(|m| {
            s(m.get("$type")).is_some_and(|t| type_matches(t, nsid::MARKDOWN))
                && m.contains_key("textBlob")
        })
    }

    /// G8 for one slug value.
    fn slug(&mut self, path: &str, v: Option<&Value>) {
        if let Some(x) = s(v)
            && !keys::is_slug(x)
        {
            self.v(RuleId::G8, path, format!("{x:?} is not a slug"));
        }
    }

    /// G8 for an array of slugs.
    fn slugs(&mut self, path: &str, v: Option<&Value>) {
        if let Some(Value::Array(a)) = v {
            for (i, x) in a.iter().enumerate() {
                self.slug(&format!("{path}[{i}]"), Some(x));
            }
        }
    }

    fn forum_ref(&mut self) {
        if let Some(f) = self.str_field("forum") {
            self.reference("$.forum", f, Some(&[Collection::Forum]));
        }
    }

    /// `fk(value.forum)`, when `forum` is a string without U+001F.
    fn forum_fk(&self) -> Option<String> {
        keys::fk(self.str_field("forum")?).ok()
    }

    fn g9(&mut self) {
        let Some(f) = self.str_field("forum") else {
            return;
        };
        match keys::fk(f) {
            Ok(fk) => {
                if keys::fk_of(self.rkey) != Some(fk.as_str()) {
                    self.v(
                        RuleId::G9,
                        "$",
                        format!("the rkey's fk segment is not fk(forum) = {fk}"),
                    );
                }
            }
            Err(e) => self.v(
                RuleId::G9,
                "$.forum",
                format!("fk(forum) is undefined: {e}"),
            ),
        }
    }

    /// The `{fk}-{tid}` key rule shared by post, wikiRevision and sequence.
    fn fk_tid_key(&mut self, rule: RuleId) {
        match keys::split_fk_tid(self.rkey) {
            None => self.key_fail(rule, "the rkey does not match ^FK-TID$"),
            Some((fk, _)) => {
                if let Some(f) = self.forum_fk()
                    && f != fk
                {
                    self.key_fail(rule, format!("the rkey's fk is not fk(forum) = {f}"));
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Collections

    fn forum(&mut self) {
        if !keys::is_slug(self.rkey) {
            self.key_fail(RuleId::FO1, "the rkey is not a slug");
        }
        self.md("$.about", self.get("about"), true, None);
        self.slugs("$.featuredTags", self.get("featuredTags"));
    }

    fn membership(&mut self) {
        if let Some(f) = self.forum_fk()
            && self.rkey != f
        {
            self.key_fail(RuleId::ME1, format!("the rkey is not fk(forum) = {f}"));
        }
    }

    fn post(&mut self) {
        self.fk_tid_key(RuleId::PO1);
        let body = self.get("body");
        self.md("$.body", body, true, None);
        self.slugs("$.tags", self.get("tags"));
        let kind = PostKind::effective(self.str_field("kind")).value;
        if kind != PostKind::QuickTake {
            match self.get("title") {
                None => self.v(RuleId::PO3, "$.title", "a title is required"),
                Some(Value::String(t)) if t.chars().all(char::is_whitespace) => self.v(
                    RuleId::PO3,
                    "$.title",
                    "the title has no non-White_Space character",
                ),
                _ => {}
            }
        } else if body.is_none() {
            self.v(RuleId::PO4, "$.body", "a quickTake requires a body");
        }
        if kind == PostKind::Link && self.get("link").is_none() {
            self.v(RuleId::PO5, "$.link", "a link post requires a link");
        }
        if Self::md_spilled(body) && self.get("excerpt").is_none() {
            self.v(
                RuleId::PO6,
                "$.excerpt",
                "a spilled body requires an excerpt",
            );
        }
    }

    fn comment(&mut self) {
        let root = self.sref("$.root", self.get("root"), &[Collection::Post]);
        if let Some(p) = self.get("parent")
            && let Some(parent) = self.sref("$.parent", Some(p), &[Collection::Comment])
            && coll_of(parent) != Some(Collection::Comment)
        {
            self.v(
                RuleId::CO3,
                "$.parent.uri",
                "parent is not an NS.comment uri",
            );
        }
        self.md("$.body", self.get("body"), false, Some(RuleId::CO4));

        let key = keys::split_comment_key(self.rkey);
        match key {
            None => self.key_fail(RuleId::CO1, "the rkey does not match ^FK-TID-TID$"),
            Some((fk, _, _)) => {
                if let Some(f) = self.forum_fk()
                    && f != fk
                {
                    self.key_fail(RuleId::CO1, format!("the rkey's fk is not fk(forum) = {f}"));
                }
            }
        }
        if let Some(root) = root {
            let problem = match (coll_of(root), rkey_of(root).and_then(keys::split_fk_tid)) {
                (Some(Collection::Post), Some((rfk, t))) => match key {
                    Some((fk, _, _)) if fk != rfk => Some(format!(
                        "root post fk {rfk} differs from the rkey's fk {fk}"
                    )),
                    Some((_, root_tid, _)) if root_tid != t => Some(format!(
                        "the rkey's root TID {root_tid} is not the root post's TID {t}"
                    )),
                    _ => None,
                },
                (Some(Collection::Post), None) => Some("root post rkey is not ^FK-TID$".into()),
                _ => Some("root.uri is not an NS.post uri".into()),
            };
            if let Some(p) = problem {
                self.key_fail(RuleId::CO2, p);
            }
        }
    }

    fn revision(&mut self) {
        const ALLOWED: &[Collection] = &[
            Collection::Post,
            Collection::Comment,
            Collection::Forum,
            Collection::Sequence,
            Collection::ModAction,
        ];
        let subject_v = self.get("subject");
        let Some(subject) = self.sref("$.subject", subject_v, ALLOWED) else {
            self.op_rule(None);
            return;
        };
        let split = keys::split_uri(subject).ok();
        let coll = split.and_then(|(_, c, _)| Collection::from_nsid(c));
        // RV1
        if split.map(|(d, _, _)| d) != Some(self.uri.did().as_str()) {
            self.key_fail(RuleId::RV1, "the subject is not in this repo");
        }
        // RV2
        if !coll.is_some_and(|c| ALLOWED.contains(&c)) {
            self.v(
                RuleId::RV2,
                "$.subject.uri",
                format!("the subject's collection is not one of {}", names(ALLOWED)),
            );
        }
        // RV3
        match keys::revision_prefix(NS, subject) {
            Ok(p) => {
                let ok = self
                    .rkey
                    .strip_prefix(p.as_str())
                    .and_then(|r| r.strip_prefix('-'))
                    .is_some_and(keys::is_tid);
                if !ok {
                    self.key_fail(RuleId::RV3, format!("the rkey is not {p}-<tid>"));
                }
            }
            Err(e) => self.key_fail(RuleId::RV3, format!("P(subject) is undefined: {e}")),
        }
        self.op_rule(coll);
        // RV5
        if let Some(record) = self.get("record") {
            let Some(rm) = record.as_object() else {
                self.v(RuleId::RV5, "$.record", "record must be an object");
                return;
            };
            let want = split.map(|(_, c, _)| c);
            match s(rm.get("$type")) {
                Some(t) if Some(t) == want => {}
                t => self.v(
                    RuleId::RV5,
                    "$.record.$type",
                    format!("record.$type {t:?} is not the subject's collection {want:?}"),
                ),
            }
            let subject_cid = s(subject_v.and_then(|v| v.get("cid")));
            match lessdugin_atp::datamodel::record_cid(record) {
                Err(e) => self.v(
                    RuleId::RV5,
                    "$.record",
                    format!("the snapshot is not valid data model: {e}"),
                ),
                Ok(cid) => {
                    if !subject_cid.is_some_and(|sc| cid_equal(&cid, sc)) {
                        self.v(
                            RuleId::RV5,
                            "$.subject.cid",
                            format!("subject.cid is not the snapshot's CID {cid}"),
                        );
                    }
                }
            }
        }
    }

    /// RV4.
    fn op_rule(&mut self, coll: Option<Collection>) {
        let Some(op) = self.str_field("op") else {
            return;
        };
        match RevisionOp::parse(op) {
            None => self.v(RuleId::RV4, "$.op", format!("unknown op {op:?}")),
            Some(RevisionOp::Delete) if coll != Some(Collection::ModAction) => self.v(
                RuleId::RV4,
                "$.op",
                "op delete is only allowed for a modAction subject",
            ),
            Some(_) => {}
        }
    }

    fn opinion(&mut self) {
        const ALLOWED: &[Collection] = &[
            Collection::Post,
            Collection::Comment,
            Collection::WikiRevision,
            Collection::Sequence,
        ];
        let subject = self.sref("$.subject", self.get("subject"), ALLOWED);
        if let Some(Value::Array(reacts)) = self.get("reacts") {
            for (i, r) in reacts.iter().enumerate() {
                self.slug(&format!("$.reacts[{i}].react"), r.get("react"));
            }
        }
        if let Some(Value::Array(tags)) = self.get("tags") {
            for (i, t) in tags.iter().enumerate() {
                self.slug(&format!("$.tags[{i}].tag"), t.get("tag"));
            }
        }
        let Some(subject) = subject else {
            return;
        };
        if !coll_of(subject).is_some_and(|c| ALLOWED.contains(&c)) {
            self.v(
                RuleId::OP1,
                "$.subject.uri",
                format!("the subject's collection is not one of {}", names(ALLOWED)),
            );
        }
        let Some(forum) = self.str_field("forum") else {
            return;
        };
        match keys::derive(NS, &RkeyInput::Opinion { forum, subject }) {
            Ok(d) if d.rkey == self.rkey => {}
            Ok(d) => self.key_fail(RuleId::OP2, format!("the rkey should be {}", d.rkey)),
            Err(e) => self.key_fail(RuleId::OP2, format!("the key is undefined: {e}")),
        }
        if let Some(f) = self.forum_fk()
            && rkey_of(subject).and_then(keys::fk_of) != Some(f.as_str())
        {
            self.key_fail(RuleId::OP2, "the subject is not in this forum (fk)");
        }
    }

    fn mod_action(&mut self) {
        // MA11 (and G4)
        if let Some(a) = self.get("adopts")
            && let Some(uri) = self.sref("$.adopts", Some(a), &[Collection::ModAction])
            && coll_of(uri) != Some(Collection::ModAction)
        {
            self.v(
                RuleId::MA11,
                "$.adopts.uri",
                "adopts is not an NS.modAction uri",
            );
        }
        let action = self.get("action").and_then(Value::as_object);
        let action_type = action.and_then(|a| s(a.get("$type")));
        let kind = action_type.and_then(ActionKind::from_type);
        let field = |k: &str| action.and_then(|a| a.get(k));

        // action-internal fields
        match kind {
            Some(ActionKind::Tier) => {
                self.md(
                    "$.action.notice",
                    field("notice"),
                    false,
                    Some(RuleId::MA12),
                );
            }
            Some(ActionKind::Lock) => {
                if let Some(Value::Array(allowed)) = field("allowed") {
                    for (i, a) in allowed.iter().enumerate() {
                        if let Some(uri) = a.as_str() {
                            self.reference(
                                &format!("$.action.allowed[{i}]"),
                                uri,
                                Some(&[Collection::Comment]),
                            );
                        }
                    }
                }
            }
            Some(ActionKind::Tag) => {
                if let Some(h) = field("head")
                    && let Some(uri) =
                        self.sref("$.action.head", Some(h), &[Collection::WikiRevision])
                {
                    let fk_ok = self
                        .forum_fk()
                        .is_none_or(|f| rkey_of(uri).and_then(keys::fk_of) == Some(f.as_str()));
                    if coll_of(uri) != Some(Collection::WikiRevision) || !fk_ok {
                        self.v(
                            RuleId::MA10,
                            "$.action.head.uri",
                            "head is not an NS.wikiRevision uri of this forum",
                        );
                    }
                }
            }
            _ => {}
        }

        // MA2: key form and derivation
        let reserved = match keys::split_mod_action_key(self.rkey) {
            Some(ModActionKey::Reserved { .. }) => {
                self.note(
                    RuleId::MA2,
                    "$",
                    "reserved {fk}-{tid} key (P2 list actions): ignored in v1",
                );
                true
            }
            Some(ModActionKey::Assertion { .. }) => {
                self.derive_mod_action_key(action);
                false
            }
            None => {
                self.key_fail(
                    RuleId::MA2,
                    "the rkey matches neither ^FK-H16$ nor ^FK-TID$",
                );
                false
            }
        };
        if reserved {
            return;
        }

        // MA7: unknown values (no effect)
        let scope = self.get("scope");
        let scope_known = match s(scope) {
            None => Some(Scope::Forum),
            Some(x) => Scope::parse(x),
        };
        if scope.is_some_and(Value::is_string) && scope_known.is_none() {
            self.note(RuleId::MA7, "$.scope", "unknown scope: no effect");
        }
        if action_type.is_some() && kind.is_none() {
            self.note(RuleId::MA7, "$.action.$type", "unknown action: no effect");
        }
        let level = s(field("level"));
        let tier = s(field("tier"));
        let pin_scope = s(field("scope"));
        match kind {
            Some(ActionKind::Access) if level.is_some_and(|l| AccessLevel::parse(l).is_none()) => {
                self.note(
                    RuleId::MA7,
                    "$.action.level",
                    "unknown access level: no effect",
                );
            }
            Some(ActionKind::Tier) if tier.is_some_and(|t| Tier::parse(t).is_none()) => {
                self.note(RuleId::MA7, "$.action.tier", "unknown tier: no effect");
            }
            Some(ActionKind::Pin) if pin_scope.is_some_and(|p| PinScope::parse(p).is_none()) => {
                self.note(
                    RuleId::MA7,
                    "$.action.scope",
                    "unknown pin scope: no effect",
                );
            }
            _ => {}
        }

        // MA1 (+ G4 for an at-uri subject, with the action's allowed collections)
        let Some(subject) = self.str_field("subject") else {
            return;
        };
        let allowed: Option<&[Collection]> = match kind {
            Some(ActionKind::Hide) => Some(&[
                Collection::Post,
                Collection::Comment,
                Collection::Sequence,
                Collection::WikiRevision,
            ]),
            Some(ActionKind::Tier) => Some(&[Collection::Post]),
            Some(ActionKind::Lock) => Some(&[Collection::Post, Collection::Forum]),
            Some(ActionKind::Pin) => match pin_scope.and_then(PinScope::parse) {
                Some(PinScope::Forum) => Some(&[Collection::Post, Collection::Sequence]),
                Some(PinScope::Thread) => Some(&[Collection::Comment]),
                None => None,
            },
            Some(ActionKind::Access | ActionKind::Restrict | ActionKind::Tag) => Some(&[]),
            None => None,
        };
        let form = if subject.starts_with("at://") {
            match self.reference("$.subject", subject, allowed) {
                Some(u)
                    if Collection::from_nsid(u.collection().as_str())
                        .is_none_or(|c| keys::matches_grammar(c, u.rkey().as_str())) =>
                {
                    SubjectForm::AtUri(u)
                }
                Some(_) => {
                    self.v(
                        RuleId::MA1,
                        "$.subject",
                        "the at-uri subject's rkey does not match its collection's grammar",
                    );
                    SubjectForm::Invalid
                }
                None => {
                    self.v(
                        RuleId::MA1,
                        "$.subject",
                        "the at-uri subject is not canonical",
                    );
                    SubjectForm::Invalid
                }
            }
        } else if subject.starts_with("did:") {
            match Did::parse(subject).and_then(|d| d.check_lessdugin_policy().map(|_| d)) {
                Ok(_) => SubjectForm::Did,
                Err(e) => {
                    self.v(
                        RuleId::MA1,
                        "$.subject",
                        format!("invalid did subject: {e}"),
                    );
                    SubjectForm::Invalid
                }
            }
        } else if let Some(slug) = subject.strip_prefix("tag:") {
            if keys::is_slug(slug) {
                SubjectForm::Tag
            } else {
                self.v(RuleId::MA1, "$.subject", format!("{slug:?} is not a slug"));
                SubjectForm::Invalid
            }
        } else {
            self.v(
                RuleId::MA1,
                "$.subject",
                "the subject is neither a did, a canonical at-uri nor tag:<slug>",
            );
            SubjectForm::Invalid
        };
        let subject_coll = match &form {
            SubjectForm::AtUri(u) => Collection::from_nsid(u.collection().as_str()),
            _ => None,
        };

        // MA3
        if let SubjectForm::AtUri(u) = &form
            && subject_coll.is_some_and(Collection::is_forum_scoped)
            && let Some(f) = self.forum_fk()
            && keys::fk_of(u.rkey().as_str()) != Some(f.as_str())
        {
            self.v(
                RuleId::MA3,
                "$.subject",
                "the subject is not in this forum (fk)",
            );
        }

        // MA4 (and MA9's no-effect annotation)
        if form != SubjectForm::Invalid {
            let ok = match kind {
                Some(ActionKind::Access | ActionKind::Restrict) => form == SubjectForm::Did,
                Some(ActionKind::Tag) => form == SubjectForm::Tag,
                Some(_) => match allowed {
                    Some(list) => subject_coll.is_some_and(|c| list.contains(&c)),
                    None => true, // unknown pin scope: no effect (MA7)
                },
                None => true,
            };
            if !ok {
                self.v(
                    RuleId::MA4,
                    "$.subject",
                    format!(
                        "this subject is not allowed for the {} action",
                        kind.map_or("unknown", |k| k.as_str())
                    ),
                );
            }
            if kind == Some(ActionKind::Lock) && subject_coll == Some(Collection::Forum) {
                self.note(
                    RuleId::MA9,
                    "$.subject",
                    "a forum-wide lock is reserved (P2): no effect",
                );
            }
        }

        // MA5: positive assertions carry subjectCid
        let positive = match kind {
            Some(ActionKind::Hide) => field("hidden").and_then(Value::as_bool) == Some(false),
            Some(ActionKind::Tier) => {
                matches!(
                    tier.and_then(Tier::parse),
                    Some(Tier::Frontpage | Tier::Curated)
                )
            }
            Some(ActionKind::Pin) => field("pinned").and_then(Value::as_bool) != Some(false),
            _ => false,
        };
        if positive && self.get("subjectCid").is_none() {
            self.v(
                RuleId::MA5,
                "$.subjectCid",
                "a positive assertion requires subjectCid",
            );
        }

        // MA6: author scope
        if scope_known == Some(Scope::Author) {
            let allowed_in_author = match kind {
                Some(ActionKind::Hide) => Some(
                    field("hidden").and_then(Value::as_bool) != Some(false)
                        && subject_coll == Some(Collection::Comment),
                ),
                Some(ActionKind::Access) => match level.and_then(AccessLevel::parse) {
                    Some(AccessLevel::Banned) => Some(true),
                    Some(AccessLevel::Member) => Some(false),
                    None => None, // unknown level: no effect (MA7)
                },
                Some(ActionKind::Pin) => match pin_scope.and_then(PinScope::parse) {
                    Some(PinScope::Thread) => Some(true),
                    Some(PinScope::Forum) => Some(false),
                    None => None,
                },
                Some(ActionKind::Lock) => Some(true),
                Some(ActionKind::Restrict | ActionKind::Tier | ActionKind::Tag) => Some(false),
                None => None,
            };
            if allowed_in_author == Some(false) {
                self.v(
                    RuleId::MA6,
                    "$.scope",
                    "this action is not allowed in author scope",
                );
            }
        }
    }

    fn derive_mod_action_key(&mut self, action: Option<&'a Map<String, Value>>) {
        let (Some(forum), Some(subject), Some(action)) =
            (self.str_field("forum"), self.str_field("subject"), action)
        else {
            return; // missing required fields fail G2
        };
        let scope = match self.get("scope") {
            None => None,
            Some(Value::String(x)) => Some(x.as_str()),
            Some(_) => return, // G2
        };
        let input = RkeyInput::ModAction {
            forum,
            scope,
            subject,
            action,
        };
        match keys::derive(NS, &input) {
            Ok(d) if d.rkey == self.rkey => {}
            Ok(d) => self.key_fail(RuleId::MA2, format!("the rkey should be {}", d.rkey)),
            Err(KeyError::UnitSeparator) => {
                self.key_fail(RuleId::MA2, "a key part contains U+001F")
            }
            Err(e) => self.key_fail(RuleId::MA2, format!("the key is undefined: {e}")),
        }
    }

    fn wiki_revision(&mut self) {
        self.fk_tid_key(RuleId::WR1);
        self.md("$.body", self.get("body"), true, None);
        self.slug("$.slug", self.get("slug"));
        self.slugs("$.parentTags", self.get("parentTags"));
        if let Some(Value::Array(parents)) = self.get("parents") {
            let f = self.forum_fk();
            for (i, p) in parents.iter().enumerate() {
                let path = format!("$.parents[{i}]");
                if let Some(uri) = self.sref(&path, Some(p), &[Collection::WikiRevision]) {
                    let fk_ok = f
                        .as_deref()
                        .is_none_or(|f| rkey_of(uri).and_then(keys::fk_of) == Some(f));
                    if coll_of(uri) != Some(Collection::WikiRevision) || !fk_ok {
                        self.v(
                            RuleId::WR2,
                            format!("{path}.uri"),
                            "parent is not an NS.wikiRevision uri of this forum",
                        );
                    }
                }
            }
        }
    }

    fn sequence(&mut self) {
        self.fk_tid_key(RuleId::SQ1);
        self.md("$.body", self.get("body"), true, None);
        if let Some(Value::Array(chapters)) = self.get("chapters") {
            for (i, ch) in chapters.iter().enumerate() {
                self.md(
                    &format!("$.chapters[{i}].body"),
                    ch.get("body"),
                    false,
                    Some(RuleId::SQ3),
                );
                if let Some(Value::Array(posts)) = ch.get("posts") {
                    for (j, p) in posts.iter().enumerate() {
                        let Some(uri) = p.as_str() else { continue };
                        let path = format!("$.chapters[{i}].posts[{j}]");
                        self.reference(&path, uri, Some(&[Collection::Post]));
                        if coll_of(uri) != Some(Collection::Post) {
                            self.v(RuleId::SQ2, path, "chapter entry is not an NS.post uri");
                        }
                    }
                }
            }
        }
    }

    fn profile(&mut self) {
        if self.rkey != keys::PROFILE_RKEY {
            self.key_fail(RuleId::PF1, "the rkey is not \"self\"");
        }
        self.md("$.bio", self.get("bio"), false, Some(RuleId::PF2));
        if let Some(Value::Array(pinned)) = self.get("pinned") {
            for (i, p) in pinned.iter().enumerate() {
                if let Some(uri) = p.as_str() {
                    self.reference(
                        &format!("$.pinned[{i}]"),
                        uri,
                        Some(&[Collection::Post, Collection::Comment, Collection::Sequence]),
                    );
                }
            }
        }
    }
}

/// `true` if the CID string `s` (any multibase) denotes `cid`.
fn cid_equal(cid: &lessdugin_atp::Cid, s: &str) -> bool {
    cid.to_string() == s || lessdugin_atp::Cid::try_from(s).is_ok_and(|c| &c == cid)
}

/// Validates only the lexicon part (G2/G3/G10-style report) of a snapshot `record` for its
/// collection: the history view renders a revision snapshot only if it passes (§6.5 "Display");
/// its rkey and reference rules do not apply.
pub fn snapshot_renderable(record: &Value) -> bool {
    let Some(t) = record.get("$type").and_then(Value::as_str) else {
        return false;
    };
    if Collection::from_nsid(t).is_none() {
        return false;
    }
    let opts = Options {
        allow_legacy_blobs: false,
        ..Options::default()
    };
    Validator::new(registry(), opts)
        .record(t, None, record)
        .is_valid()
}
