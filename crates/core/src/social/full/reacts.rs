// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: react aggregation (`karma/1` with the `default/1` palette, `social.md` §9).
//!
//! ```text
//! per counting opinion v with the reacts axis enabled (§5):
//!   entries(v) = [ e ∈ v.reacts : e.react ∈ palette ∧ e.stance ∈ {for, against} ]  // default for
//!   qkey(e)    = e.quote ? norm(trim_ws(exact)) (lfm-1.md §11) : ""
//!   group by (react, qkey): both stances → dropped; else one (react, q, stance)
//! per subject s and (react, q):
//!   Fv = voters ∈ E with for; Av = voters ∈ E with against
//!   C  = { u ∈ Fv : passes(u, reactCreate) };  valid ⇔ C ≠ ∅
//!   creator = argmax K over C, ties → smallest DID
//!   supporters = { u ∈ Fv : passes(u, reactSecond) } ∪ {creator};  opposers = { u ∈ Av : passes(u, reactAnti) }
//! ```
//!
//! Inputs: `cx` (opinion values for `reacts`, the palette), `a` (L0), `karma` (E, K, counting
//! opinions with the reacts axis), `gates` (the L4 stage). `qkey` is
//! [`lessdugin_markdown::quote::qkey`] (identical to `lessdugin_lexicon::records::qkey`; both
//! run `testdata/vectors/lfm1/quote.json`).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::values::{ReactPalette, Stance};
use lessdugin_markdown::quote::qkey;
use serde_json::Value;

use crate::social::full::cx::Cx;
use crate::social::full::gates::{Gate, Gates, passes};
use crate::social::full::karma::Karma;
use crate::social::full::view::Stage;

/// The react ids of the `default/1` palette (§9.1, `docs/spec/reacts-default-1.json`), in
/// palette order.
pub const DEFAULT_1: [&str; 34] = [
    "agree",
    "disagree",
    "important",
    "insightful",
    "changed-mind",
    "crux",
    "not-a-crux",
    "locally-valid",
    "locally-invalid",
    "typo",
    "wrong",
    "misleading",
    "unclear",
    "dont-understand",
    "examples",
    "citation-needed",
    "concrete",
    "overconfident",
    "underconfident",
    "hits-the-mark",
    "missed-the-point",
    "strawman",
    "steelman",
    "scout-mindset",
    "soldier-mindset",
    "too-harsh",
    "good-point",
    "thanks",
    "seen",
    "will-reply-later",
    "laugh",
    "heart",
    "surprise",
    "empathy",
];

/// The react ids of an effective palette.
pub fn palette(p: ReactPalette) -> &'static [&'static str] {
    match p {
        ReactPalette::Default1 => &DEFAULT_1,
    }
}

/// One valid react on a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactTally {
    /// The palette id.
    pub react: String,
    /// The quote key (`""` for the whole item).
    pub qkey: String,
    /// The creator's DID.
    pub creator: String,
    /// The supporters, the creator included.
    pub supporters: BTreeSet<String>,
    /// The opposers.
    pub opposers: BTreeSet<String>,
    /// The creator's quote (display only): the first entry of the creator's group.
    pub quote: Option<Quote>,
}

/// A react's `quote` (`defs#textQuote`), typed, so that its serialised form does not depend on
/// the key order of the source JSON (§1.3 rule 6). Unknown fields are not kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quote {
    /// `exact`.
    pub exact: String,
    /// `prefix`.
    pub prefix: Option<String>,
    /// `suffix`.
    pub suffix: Option<String>,
    /// `start` (a code-point offset hint).
    pub start: Option<i64>,
}

impl Quote {
    /// Reads a `defs#textQuote` value (fields of other types are left out).
    pub fn of(v: &Value) -> Self {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Self {
            exact: text("exact").unwrap_or_default(),
            prefix: text("prefix"),
            suffix: text("suffix"),
            start: v.get("start").and_then(Value::as_i64),
        }
    }
}

/// One opinion's group for a (react, qkey): its stances and the first entry's quote.
#[derive(Default)]
struct OwnGroup {
    pro: bool,
    anti: bool,
    quote: Option<Quote>,
}

/// The entries of one opinion (OP3, then grouping by (react, qkey)): `(react, q) → (stance,
/// first quote)`, contradictions dropped.
pub fn entries(value: &Value, ids: &[&str]) -> BTreeMap<(String, String), (Stance, Option<Quote>)> {
    let mut groups: BTreeMap<(String, String), OwnGroup> = BTreeMap::new();
    for e in value
        .get("reacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(react) = e.get("react").and_then(Value::as_str) else {
            continue;
        };
        if !ids.contains(&react) {
            continue;
        }
        let stance = match e.get("stance") {
            None => Some(Stance::For),
            Some(s) => s.as_str().and_then(Stance::parse),
        };
        let Some(stance) = stance else { continue };
        let quote = e.get("quote").filter(|q| !q.is_null());
        let q = quote.map_or_else(String::new, |q| {
            qkey(Some(
                q.get("exact").and_then(Value::as_str).unwrap_or_default(),
            ))
        });
        let g = groups
            .entry((react.to_owned(), q))
            .or_insert_with(|| OwnGroup {
                quote: quote.map(Quote::of),
                ..OwnGroup::default()
            });
        match stance {
            Stance::For => g.pro = true,
            Stance::Against => g.anti = true,
        }
    }
    groups
        .into_iter()
        .filter_map(|(k, g)| match (g.pro, g.anti) {
            (true, false) => Some((k, (Stance::For, g.quote))),
            (false, true) => Some((k, (Stance::Against, g.quote))),
            _ => None,
        })
        .collect()
}

/// The voters of one (subject, react, qkey) in E: for (with their first quote) and against.
#[derive(Default)]
struct Tally {
    pro: BTreeMap<String, Option<Quote>>,
    anti: BTreeSet<String>,
}

/// L5 (`social.md` §9.2): the valid reacts of every subject with one, each list sorted by
/// (react, qkey) bytes.
pub fn reacts(
    cx: &Cx<'_>,
    a: &BTreeMap<String, u8>,
    karma: &Stage<Karma>,
    gates: &Stage<Gates>,
) -> Stage<BTreeMap<String, Vec<ReactTally>>> {
    let k = karma.as_ref().map_err(|p| *p)?;
    gates.as_ref().map_err(|p| *p)?;
    let ids = palette(cx.policy.react_palette);
    let mut tallies: BTreeMap<&str, BTreeMap<(String, String), Tally>> = BTreeMap::new();
    for (uri, o) in &k.opinions {
        // the contradiction is decided within the opinion; voters outside E are then dropped
        if !o.reacts || !k.e.contains(&o.voter) {
            continue;
        }
        let Some(value) = cx.skel(uri).and_then(|x| x.value()) else {
            continue;
        };
        for (key, (stance, quote)) in entries(value, ids) {
            let t = tallies
                .entry(o.subject.as_str())
                .or_default()
                .entry(key)
                .or_default();
            match stance {
                Stance::For => {
                    t.pro.insert(o.voter.clone(), quote);
                }
                Stance::Against => {
                    t.anti.insert(o.voter.clone());
                }
            }
        }
    }
    let pass = |d: &str, g: Gate| passes(&cx.policy, a, k, d, g);
    let mut out: BTreeMap<String, Vec<ReactTally>> = BTreeMap::new();
    for (subject, groups) in tallies {
        let mut list = Vec::new();
        for ((react, q), t) in groups {
            // C, then the greatest K, ties by the smallest DID (BTreeMap order: first wins)
            let mut creator: Option<(&String, i64)> = None;
            for d in t.pro.keys().filter(|d| pass(d, Gate::ReactCreate)) {
                let kd = k.k_of(d);
                if creator.is_none_or(|(_, best)| kd > best) {
                    creator = Some((d, kd));
                }
            }
            let Some((creator, _)) = creator else {
                continue;
            };
            let mut supporters: BTreeSet<String> = t
                .pro
                .keys()
                .filter(|d| pass(d, Gate::ReactSecond))
                .cloned()
                .collect();
            supporters.insert(creator.clone());
            let opposers = t
                .anti
                .iter()
                .filter(|d| pass(d, Gate::ReactAnti))
                .cloned()
                .collect();
            list.push(ReactTally {
                react,
                qkey: q,
                creator: creator.clone(),
                supporters,
                opposers,
                quote: t.pro.get(creator).cloned().flatten(),
            });
        }
        if !list.is_empty() {
            out.insert(subject.to_owned(), list);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_matches_the_json() {
        let json: Value = serde_json::from_str(include_str!(
            "../../../../../docs/spec/reacts-default-1.json"
        ))
        .unwrap();
        let ids: Vec<&str> = json["reacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, DEFAULT_1);
        assert_eq!(json["id"], "default/1");
    }

    #[test]
    fn contradictions_and_collapse() {
        let v = serde_json::json!({"reacts": [
            {"react": "crux"},
            {"react": "crux", "stance": "against"},
            {"react": "typo", "stance": "against"},
            {"react": "typo", "quote": {"exact": " teh "}},
            {"react": "typo", "quote": {"exact": "teh"}, "stance": "for"},
            {"react": "nope"},
            {"react": "seen", "stance": "maybe"},
        ]});
        let e = entries(&v, &DEFAULT_1);
        let keys: Vec<(&str, &str, Stance)> = e
            .iter()
            .map(|((r, q), (s, _))| (r.as_str(), q.as_str(), *s))
            .collect();
        assert_eq!(
            keys,
            [("typo", "", Stance::Against), ("typo", "teh", Stance::For)]
        );
        // the first entry's quote
        assert_eq!(
            e[&("typo".to_owned(), "teh".to_owned())].1,
            Some(Quote {
                exact: " teh ".into(),
                ..Quote::default()
            })
        );
    }
}
