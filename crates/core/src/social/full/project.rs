// SPDX-License-Identifier: AGPL-3.0-or-later
//! The vector projection of a [`ForumView`] (`testdata/vectors/README.md`, "Social fold
//! vectors"): one name and one JSON shape per field, across every kind. Runners (the Rust
//! testkit, the wasm `vectors` build) compare each field a vector's `expected` lists with
//! [`project`].
//!
//! Sets are arrays sorted by UTF-8 bytes; integer maps keyed by DID list only non-zero entries; a
//! field whose layer is a stub projects to [`ProjectError::Pending`].

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::values::{AccessLevel, MembershipMode};
use serde_json::{Map, Value, json};

use crate::social::digest;
use crate::social::full::gates::Gate;
use crate::social::full::view::{
    DimValue, ForumView, HiddenReason, ModStatus, Pending, QueueItem, Resolution, Stage, State,
};
use crate::social::input::FoldInput;
use crate::social::threads::Parent;

/// Why a field could not be projected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The field's layer is a stub.
    Pending(Pending),
    /// No such field.
    UnknownField(String),
    /// The field is a function of the fold input, which the view does not keep
    /// (`digestPreimage`): use [`project_fold`].
    NeedsInput(String),
}

impl From<Pending> for ProjectError {
    fn from(p: Pending) -> Self {
        ProjectError::Pending(p)
    }
}

/// Every field name [`project`] knows.
pub const FIELDS: &[&str] = &[
    "degraded",
    "newerRulesBanner",
    "tDecl",
    "times",
    "authorities",
    "mode",
    "members",
    "P",
    "G",
    "B",
    "N",
    "S",
    "applicants",
    "newcomers",
    "resolved",
    "tiers",
    "modStates",
    "modLog",
    "modQueue",
    "rootMissing",
    "parent",
    "missing",
    "depth",
    "orphaned",
    "maxDepth",
    "inputDigest",
    "digestPreimage",
    "algs",
    "params",
    "states",
    "displayed",
    "postOrder",
    "hotBits",
    "commentOrder",
    "recentDiscussion",
    "counting",
    "K1",
    "seedUp",
    "KP",
    "K",
    "E",
    "established",
    "scores",
    "gateValues",
    "passes",
    "reacts",
    "wiki",
    "tagRel",
    "applies",
];

fn stage<T>(s: &Stage<T>) -> Result<&T, ProjectError> {
    s.as_ref().map_err(|p| ProjectError::Pending(*p))
}

fn set<'a>(s: impl IntoIterator<Item = &'a String>) -> Value {
    let mut v: Vec<&String> = s.into_iter().collect();
    v.sort();
    v.dedup();
    json!(v)
}

fn nonzero(m: &BTreeMap<String, i64>) -> Value {
    Value::Object(
        m.iter()
            .filter(|(_, v)| **v != 0)
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect(),
    )
}

/// A visibility state: `{state, reason?, removed?}`.
pub fn state_json(s: &State) -> Value {
    let hidden = |name: &str, r: &HiddenReason| {
        let mut o = Map::new();
        o.insert("state".into(), json!(name));
        o.insert("reason".into(), json!(r.name()));
        if let HiddenReason::Removed(rm) = r {
            o.insert(
                "removed".into(),
                json!({
                    "rank": rm.rank,
                    "actor": rm.actor,
                    "spam": rm.spam,
                    "reason": rm.reason,
                }),
            );
        }
        Value::Object(o)
    };
    match s {
        State::Excluded => json!({"state": "Excluded"}),
        State::Visible => json!({"state": "Visible"}),
        State::Collapsed(r) => json!({"state": "Collapsed", "reason": r.name()}),
        State::Hidden(r) => hidden("Hidden", r),
        State::Tombstone(r) => hidden("Tombstone", r),
    }
}

fn dim_value_json(v: &DimValue) -> Value {
    match v {
        DimValue::Bool(b) => json!(b),
        DimValue::Access(l) => json!(match l {
            AccessLevel::Banned => "banned",
            AccessLevel::Member => "member",
        }),
        DimValue::Tier(t) => json!(t.as_str()),
        DimValue::Caps(c) => {
            let mut names: Vec<&str> = c.iter().map(|c| c.as_str()).collect();
            names.sort_unstable();
            json!(names)
        }
        DimValue::Head { uri, cid } => json!({"uri": uri, "cid": cid}),
    }
}

fn resolution_json(r: &Resolution) -> Value {
    let mut o = Map::new();
    o.insert("value".into(), dim_value_json(&r.value));
    o.insert("rank".into(), json!(r.rank));
    o.insert("winner".into(), json!(r.winner));
    if let Some(a) = &r.allowed {
        o.insert("allowed".into(), set(a));
    }
    Value::Object(o)
}

fn status_json(uri: &str, st: ModStatus) -> Value {
    match st {
        ModStatus::Overridden { rank } => {
            json!({"uri": uri, "status": st.name(), "rank": rank})
        }
        _ => json!({"uri": uri, "status": st.name()}),
    }
}

fn thread_field(v: &ForumView, field: &str) -> Value {
    let p = v.query.thread.as_deref().unwrap_or_default();
    let Some(pl) = v.threads.get(p) else {
        return match field {
            "rootMissing" => json!(true),
            "parent" | "depth" => json!({}),
            "maxDepth" => json!(0),
            _ => json!([]),
        };
    };
    match field {
        "rootMissing" => json!(pl.root_missing),
        "parent" => Value::Object(
            pl.parent
                .iter()
                .map(|(c, par)| {
                    let to = match par {
                        Parent::Root => p,
                        Parent::Comment(u) | Parent::Missing(u) => u.as_str(),
                    };
                    (c.clone(), json!(to))
                })
                .collect(),
        ),
        "missing" => set(&pl.missing),
        "depth" => json!(pl.depth),
        "orphaned" => set(&pl.orphaned),
        _ => json!(pl.max_depth()),
    }
}

/// Projects `field` of the fold of `input` into `v` (the README's "Comparing results" contract):
/// [`project`], plus `digestPreimage`, which the view does not keep (the preimage D of §1.2 is as
/// large as the input's uri list).
pub fn project_fold(input: &FoldInput, v: &ForumView, field: &str) -> Result<Value, ProjectError> {
    match field {
        "digestPreimage" => {
            let d = digest::preimage(input).map_err(|e| ProjectError::NeedsInput(e.to_string()))?;
            Ok(json!(String::from_utf8_lossy(&d)))
        }
        _ => project(v, field),
    }
}

/// Projects `field` of `v` (the README's "Comparing results" contract); `digestPreimage` needs
/// the input ([`project_fold`]).
pub fn project(v: &ForumView, field: &str) -> Result<Value, ProjectError> {
    let l2 = &v.participation;
    Ok(match field {
        "degraded" => json!(v.degraded),
        "newerRulesBanner" => json!(v.newer_rules_banner),

        // time
        "tDecl" => json!(v.t_decl),
        "times" => Value::Object(
            v.times
                .iter()
                .map(|(u, t)| {
                    (
                        u.clone(),
                        json!({
                            "tRank": t.t_rank,
                            "futureDated": t.future,
                            "inconsistentTimestamps": t.inconsistent,
                        }),
                    )
                })
                .collect(),
        ),

        // modfold
        "authorities" => json!(v.authorities),
        "mode" => json!(match l2.mode {
            MembershipMode::Approval => "approval",
            MembershipMode::Open => "open",
        }),
        "members" => set(&l2.members),
        "P" => set(&l2.p),
        "G" => set(&l2.g),
        "B" => set(&l2.b),
        "N" => set(&l2.n),
        "S" => set(&l2.s),
        "applicants" => set(&l2.applicants),
        "newcomers" => set(stage(&v.newcomers)?),
        "resolved" => Value::Object(
            v.moderation
                .resolved
                .iter()
                .map(|(s, dims)| {
                    let o: Map<String, Value> = dims
                        .iter()
                        .map(|(d, r)| (d.name().to_owned(), resolution_json(r)))
                        .collect();
                    (s.clone(), Value::Object(o))
                })
                .collect(),
        ),
        "tiers" => Value::Object(
            v.tiers
                .iter()
                .map(|(u, t)| (u.clone(), json!(t.as_str())))
                .collect(),
        ),
        "modStates" => Value::Object(
            stage(&v.mod_states)?
                .iter()
                .map(|(u, s)| (u.clone(), s.as_ref().map_or(Value::Null, state_json)))
                .collect(),
        ),
        "modLog" => json!(
            stage(&v.mod_log)?
                .iter()
                .map(|e| status_json(&e.uri, e.status))
                .collect::<Vec<_>>()
        ),
        "modQueue" => json!(
            v.mod_queue
                .iter()
                .map(|q| match q {
                    QueueItem::Applicant(d) => json!({"type": "applicant", "did": d}),
                    QueueItem::Conflict(s) => json!({"type": "conflict", "subject": s}),
                    QueueItem::Stale(u) => json!({"type": "stale", "uri": u}),
                })
                .collect::<Vec<_>>()
        ),

        // thread (query.thread)
        "rootMissing" | "parent" | "missing" | "depth" | "orphaned" | "maxDepth" => {
            thread_field(v, field)
        }

        // digest
        "inputDigest" => json!(v.input_digest),
        "digestPreimage" => return Err(ProjectError::NeedsInput(field.to_owned())),
        "algs" => json!(v.policy.algs_line()),
        "params" => json!(v.policy.params_line()),

        // visibility
        "states" => Value::Object(
            stage(&v.visibility)?
                .states
                .iter()
                .map(|(u, s)| (u.clone(), state_json(s)))
                .collect(),
        ),
        "displayed" => Value::Object(
            stage(&v.visibility)?
                .displayed
                .iter()
                .map(|(u, s)| (u.clone(), state_json(s)))
                .collect(),
        ),

        // rank
        "postOrder" => json!(stage(&v.orderings)?.post_order),
        "hotBits" => Value::Object(
            stage(&v.orderings)?
                .hot_bits
                .iter()
                .map(|(u, b)| (u.clone(), json!(format!("{b:016x}"))))
                .collect(),
        ),
        "commentOrder" => json!(stage(&v.orderings)?.comment_order),
        "recentDiscussion" => json!(
            stage(&v.orderings)?
                .recent_discussion
                .iter()
                .map(|r| json!({"post": r.post, "comments": r.comments}))
                .collect::<Vec<_>>()
        ),

        // karma
        "counting" => set(&stage(&v.karma)?.counting),
        "K1" => nonzero(&stage(&v.karma)?.k1),
        "seedUp" => nonzero(&stage(&v.karma)?.seed_up),
        "KP" => nonzero(&stage(&v.karma)?.kp),
        "K" => nonzero(&stage(&v.karma)?.k),
        "E" => set(&stage(&v.karma)?.e),
        "established" => {
            let e = &stage(&v.karma)?.e;
            set(e.iter().filter(|d| !l2.s.contains(*d)))
        }
        "scores" => Value::Object(
            stage(&v.scores)?
                .iter()
                .map(|(u, s)| {
                    (
                        u.clone(),
                        json!({
                            "score": s.score,
                            "agreement": s.agreement,
                            "unweighted": s.unweighted,
                            "earlier": s.earlier,
                            "weighted": s.weighted,
                        }),
                    )
                })
                .collect(),
        ),
        "gateValues" => Value::Object(
            Gate::ALL
                .iter()
                .map(|g| (g.name().to_owned(), json!(g.value(&v.policy))))
                .collect(),
        ),
        "passes" => Value::Object(
            stage(&v.gates)?
                .passes
                .iter()
                .map(|(d, gs)| {
                    let o: Map<String, Value> = gs
                        .iter()
                        .map(|(g, b)| (g.name().to_owned(), json!(b)))
                        .collect();
                    (d.clone(), Value::Object(o))
                })
                .collect(),
        ),

        // reacts
        "reacts" => Value::Object(
            stage(&v.reacts)?
                .iter()
                .filter(|(_, list)| !list.is_empty())
                .map(|(s, list)| {
                    let l: Vec<Value> = list
                        .iter()
                        .map(|r| {
                            json!({
                                "react": r.react,
                                "qkey": r.qkey,
                                "creator": r.creator,
                                "supporters": set(&r.supporters),
                                "opposers": set(&r.opposers),
                            })
                        })
                        .collect();
                    (s.clone(), json!(l))
                })
                .collect(),
        ),

        // tags
        "wiki" => Value::Object(
            stage(&v.wiki)?
                .pages
                .iter()
                .map(|(slug, pg)| {
                    let cands: Map<String, Value> = pg
                        .candidates
                        .iter()
                        .map(|(u, c)| {
                            (
                                u.clone(),
                                json!({
                                    "depth": c.depth,
                                    "parents": set(&c.parents),
                                    "altered": set(&c.altered),
                                    "parentUnavailable": c.parent_unavailable,
                                }),
                            )
                        })
                        .collect();
                    (
                        slug.clone(),
                        json!({
                            "canonical": pg.canonical,
                            "exists": pg.exists,
                            "applicable": pg.applicable,
                            "partial": pg.partial,
                            "history": pg.history,
                            "unmerged": pg.unmerged,
                            "candidates": cands,
                        }),
                    )
                })
                .collect(),
        ),
        "tagRel" => Value::Object(
            stage(&v.wiki)?
                .rel
                .iter()
                .filter(|(_, m)| m.values().any(|r| *r != 0))
                .map(|(p, m)| (p.clone(), nonzero(m)))
                .collect(),
        ),
        "applies" => Value::Object(
            stage(&v.wiki)?
                .applies
                .iter()
                .filter(|(_, s)| !s.is_empty())
                .map(|(p, s): (&String, &BTreeSet<String>)| (p.clone(), set(s)))
                .collect(),
        ),

        other => return Err(ProjectError::UnknownField(other.to_owned())),
    })
}
