// SPDX-License-Identifier: MIT OR Apache-2.0
//! `opinion` (§6.6) and its deterministic write function `apply` (§8.5).

use std::collections::BTreeSet;

use lessdugin_atp::syntax::{AtUri, Datetime};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;

use super::{StrongRef, TextQuote};
use crate::collection::nsid;
use crate::values::{Agreement, Stance, VoteValue};

fn opinion_type() -> String {
    nsid::OPINION.to_owned()
}

/// `NS.opinion` (rkey `{fk}-{stid}-{h16("opinion", subject.uri)}`): one per (voter, subject).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opinion {
    /// `NS.opinion` of this build.
    #[serde(rename = "$type", default = "opinion_type")]
    pub r#type: String,
    /// The judged item; `cid` is the version the voter judged.
    pub subject: StrongRef,
    /// The forum at-uri.
    pub forum: AtUri,
    /// Karma axis, `kv{up, down, strongUp, strongDown}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub karma: Option<String>,
    /// Agreement axis, `kv{agree, disagree, strongAgree, strongDisagree}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreement: Option<String>,
    /// Reacts (≤ 32), at most one per `(react, qkey(quote))`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reacts: Option<Vec<React>>,
    /// Tag votes (≤ 32), at most one per tag; read only for post subjects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagVote>>,
    /// Creation time.
    pub created_at: Datetime,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `opinion#react`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct React {
    /// React id (a slug from the forum's palette).
    pub react: String,
    /// `kv{for, against}`, default `for`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,
    /// The quoted text the react applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<TextQuote>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl React {
    /// The entry's dedup key `(react, qkey(quote))`.
    pub fn key(&self) -> (&str, String) {
        (self.react.as_str(), qkey(self.quote.as_ref()))
    }

    /// The stance (absent → `for`; unknown → `None`, the entry is discarded, OP3).
    pub fn stance(&self) -> Option<Stance> {
        Stance::effective(self.stance.as_deref())
    }
}

/// `opinion#tagVote`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagVote {
    /// Tag slug.
    pub tag: String,
    /// `kv{up, down, strongUp, strongDown}`.
    pub value: String,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TagVote {
    /// The value, if known (an unknown value discards the entry, OP3).
    pub fn value(&self) -> Option<VoteValue> {
        VoteValue::parse(&self.value)
    }
}

/// `qkey(quote)` (`lfm-1.md` §11): `norm(trim_ws(quote.exact))`, where `norm` is NFC followed by
/// replacing every run of `White_Space` characters with one U+0020; `""` without a quote.
pub fn qkey(quote: Option<&TextQuote>) -> String {
    let Some(q) = quote else {
        return String::new();
    };
    let trimmed = q.exact.trim_matches(char::is_whitespace);
    let mut out = String::with_capacity(trimmed.len());
    let mut in_ws = false;
    for c in trimmed.nfc() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// One opinion intent (§8.5), applied in order by [`Opinion::apply`].
#[derive(Debug, Clone, PartialEq)]
pub enum OpinionIntent {
    /// `setKarma(x | clear)`: `None` removes the karma axis.
    Karma(Option<VoteValue>),
    /// `setAgreement(x | clear)`: `None` removes the agreement axis.
    Agreement(Option<Agreement>),
    /// `react(add, r, quote?, stance)`: replaces the entry with key `(r, qkey(quote))` in place,
    /// or appends one.
    AddReact {
        /// React id.
        react: String,
        /// Optional quote.
        quote: Option<TextQuote>,
        /// Stance (written explicitly).
        stance: Stance,
    },
    /// `react(remove, r, quote?)`: removes the entry with key `(r, qkey(quote))`.
    RemoveReact {
        /// React id.
        react: String,
        /// Optional quote (only its `qkey` matters).
        quote: Option<TextQuote>,
    },
    /// `tag(set, slug, value | clear)`: replaces in place, appends, or (with `None`) removes the
    /// entry for `tag`.
    Tag {
        /// Tag slug.
        tag: String,
        /// The value, or `None` to clear.
        value: Option<VoteValue>,
    },
}

impl Opinion {
    /// A new, empty opinion: `{$type, subject, forum, createdAt}`.
    pub fn new(subject: StrongRef, forum: AtUri, created_at: Datetime) -> Self {
        Self {
            r#type: opinion_type(),
            subject,
            forum,
            karma: None,
            agreement: None,
            reacts: None,
            tags: None,
            created_at,
            updated_at: None,
            extra: Map::new(),
        }
    }

    /// `apply(v, intents)` (§8.5), a pure function. Starts from `current` (unknown fields kept),
    /// or a new opinion on `subject` in `forum` created `now`; applies `intents` in order; sets
    /// `subject.cid` to `subject_cid` (the version the user saw) and, if `current` existed,
    /// `updatedAt = now`.
    ///
    /// **Array order** (deterministic bytes): a replaced entry keeps its position, an inserted
    /// entry is appended, removal keeps the order of the rest. Later duplicates of a key are
    /// dropped (a writer never writes two entries with the same key), and an array emptied by an
    /// intent is removed (`empty()` treats `[]` as absent either way).
    pub fn apply(
        current: Option<&Opinion>,
        subject: &AtUri,
        forum: &AtUri,
        subject_cid: &str,
        intents: &[OpinionIntent],
        now: &Datetime,
    ) -> Opinion {
        let mut v = match current {
            Some(c) => c.clone(),
            None => Opinion::new(
                StrongRef::new(subject.clone(), subject_cid),
                forum.clone(),
                now.clone(),
            ),
        };
        for intent in intents {
            v.apply_intent(intent);
        }
        v.dedupe();
        v.subject.cid = subject_cid.to_owned();
        if current.is_some() {
            v.updated_at = Some(now.clone());
        }
        v
    }

    /// Applies one intent in place (array-order rules as in [`Opinion::apply`]).
    pub fn apply_intent(&mut self, intent: &OpinionIntent) {
        match intent {
            OpinionIntent::Karma(k) => self.karma = k.map(|k| k.as_str().to_owned()),
            OpinionIntent::Agreement(a) => self.agreement = a.map(|a| a.as_str().to_owned()),
            OpinionIntent::AddReact {
                react,
                quote,
                stance,
            } => {
                let key = (react.as_str(), qkey(quote.as_ref()));
                let reacts = self.reacts.get_or_insert_with(Vec::new);
                match reacts.iter_mut().find(|r| r.key() == key) {
                    Some(r) => {
                        r.stance = Some(stance.as_str().to_owned());
                        r.quote.clone_from(quote);
                    }
                    None => reacts.push(React {
                        react: react.clone(),
                        stance: Some(stance.as_str().to_owned()),
                        quote: quote.clone(),
                        extra: Map::new(),
                    }),
                }
            }
            OpinionIntent::RemoveReact { react, quote } => {
                let key = (react.as_str(), qkey(quote.as_ref()));
                if let Some(reacts) = &mut self.reacts {
                    let before = reacts.len();
                    reacts.retain(|r| r.key() != key);
                    if reacts.is_empty() && before > 0 {
                        self.reacts = None;
                    }
                }
            }
            OpinionIntent::Tag { tag, value } => match value {
                Some(val) => {
                    let tags = self.tags.get_or_insert_with(Vec::new);
                    match tags.iter_mut().find(|t| &t.tag == tag) {
                        Some(t) => t.value = val.as_str().to_owned(),
                        None => tags.push(TagVote {
                            tag: tag.clone(),
                            value: val.as_str().to_owned(),
                            extra: Map::new(),
                        }),
                    }
                }
                None => {
                    if let Some(tags) = &mut self.tags {
                        let before = tags.len();
                        tags.retain(|t| &t.tag != tag);
                        if tags.is_empty() && before > 0 {
                            self.tags = None;
                        }
                    }
                }
            },
        }
    }

    /// Drops later entries whose key repeats an earlier one (first occurrence wins). O(n log n)
    /// (ordered sets: no hashing randomness): the current opinion is fetched and may be large.
    fn dedupe(&mut self) {
        if let Some(reacts) = &mut self.reacts {
            let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
            reacts.retain(|r| {
                let (a, b) = r.key();
                seen.insert((a.to_owned(), b))
            });
        }
        if let Some(tags) = &mut self.tags {
            let mut seen: BTreeSet<String> = BTreeSet::new();
            tags.retain(|t| seen.insert(t.tag.clone()));
        }
    }

    /// `empty(v)` (§8.5): no field other than `$type`, `subject`, `forum`, `createdAt` and
    /// `updatedAt`, where an empty `reacts` or `tags` array counts as absent. Unknown fields
    /// count as content (§14 item 13). An empty opinion is deleted, not written.
    pub fn is_empty(&self) -> bool {
        self.karma.is_none()
            && self.agreement.is_none()
            && self.reacts.as_ref().is_none_or(Vec::is_empty)
            && self.tags.as_ref().is_none_or(Vec::is_empty)
            && self.extra.is_empty()
    }

    /// `equalIgnoringUpdatedAt` (§8.5).
    pub fn eq_ignoring_updated_at(&self, other: &Opinion) -> bool {
        let mut a = self.clone();
        a.updated_at.clone_from(&other.updated_at);
        &a == other
    }

    /// The karma axis, if known (an unknown value makes the axis absent, OP3).
    pub fn karma(&self) -> Option<VoteValue> {
        self.karma.as_deref().and_then(VoteValue::parse)
    }

    /// The agreement axis, if known (OP3).
    pub fn agreement(&self) -> Option<Agreement> {
        self.agreement.as_deref().and_then(Agreement::parse)
    }
}
