// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs the social fold vectors the core implements so far (`testdata/vectors/README.md`,
//! "Social fold vectors"): `thread/*.json` (§11 placement), `time/*.json` (§2), and the
//! `algs`/`params`/`inputDigest` fields of `fold/digest.json` (§1.2).
//!
//! **Namespace.** The vectors are written in the final namespace (`ns` in each file). With the
//! `final-ns` feature (`cargo test -p lessdugin-core --features final-ns --test social_vectors`)
//! this build's lexicon is that namespace and record validity (rule V1) is computed exactly. In a
//! default (`.temp`) build each record is checked after mapping its NSIDs into this build's
//! namespace, ignoring the rules whose outcome depends on keys hashed from the namespace (G5, G9,
//! the key equalities and RV5): the vectors' deliberately invalid records break other rules.
//! The fold itself always runs on the original values with the file's `ns`.

use std::path::PathBuf;

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::social::digest;
use lessdugin_core::social::fold::fold;
use lessdugin_core::social::input::{FoldInput, ForumInput};
use lessdugin_core::social::threads::Parent;
use lessdugin_core::social::time;
use lessdugin_lexicon::RuleId;
use lessdugin_lexicon::ns::{NS, to_build_nsid};
use serde_json::{Map, Value, json};

fn load(rel: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/vectors")
        .join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// Rules decided by keys that hash the namespace (not checkable after a prefix mapping).
const NS_KEYED: [RuleId; 15] = [
    RuleId::G5,
    RuleId::G9,
    RuleId::ME1,
    RuleId::PO1,
    RuleId::CO1,
    RuleId::CO2,
    RuleId::RV3,
    RuleId::RV5,
    RuleId::OP2,
    RuleId::MA2,
    RuleId::MA3,
    RuleId::MA10,
    RuleId::WR1,
    RuleId::WR2,
    RuleId::SQ1,
];

fn map_str(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("at://") {
        let mut parts: Vec<String> = rest.split('/').map(str::to_owned).collect();
        if parts.len() >= 2
            && let Some(c) = to_build_nsid(&parts[1])
        {
            parts[1] = c;
        }
        return format!("at://{}", parts.join("/"));
    }
    to_build_nsid(s).unwrap_or_else(|| s.to_owned())
}

fn map_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(map_str(s)),
        Value::Array(a) => Value::Array(a.iter().map(map_value).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), map_value(v)))
                .collect::<Map<String, Value>>(),
        ),
        other => other.clone(),
    }
}

/// `data-model.md` §7 for a vector record in namespace `ns`.
fn validity(ns: &str, uri: &str, value: &Value) -> (bool, Option<String>) {
    let exact = ns == NS;
    let (uri, value) = if exact {
        (uri.to_owned(), value.clone())
    } else {
        (map_str(uri), map_value(value))
    };
    let Ok(u) = AtUri::parse(&uri) else {
        return (false, Some("G4".into()));
    };
    let v = lessdugin_lexicon::validity::check_uri(&u, &value);
    let failed: Vec<RuleId> = v
        .violations
        .iter()
        .map(|x| x.rule_id)
        .filter(|r| exact || !NS_KEYED.contains(r))
        .collect();
    (failed.is_empty(), failed.first().map(|r| r.to_string()))
}

struct Case {
    file: String,
    name: String,
    ns: String,
    input: Value,
    expected: Map<String, Value>,
}

fn cases(rel: &str) -> Vec<Case> {
    let f = load(rel);
    let ns = f["ns"].as_str().unwrap_or(NS).to_owned();
    f["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| Case {
            file: rel.to_owned(),
            name: v["name"].as_str().unwrap().to_owned(),
            ns: ns.clone(),
            input: v["input"].clone(),
            expected: v["expected"].as_object().unwrap().clone(),
        })
        .collect()
}

fn forum_input(c: &Case) -> (FoldInput, ForumInput) {
    let fi = FoldInput::from_vector_json(&c.input["fold"]).unwrap();
    let ns = c.ns.clone();
    let input = ForumInput::from_contract(&fi, &c.ns, |u, v| validity(&ns, u, v)).unwrap();
    (fi, input)
}

/// Compares every listed field; returns the failures.
fn compare(c: &Case, actual: &Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for (k, want) in &c.expected {
        match actual.get(k) {
            Some(got) if got == want => {}
            Some(got) => out.push(format!(
                "{} / {}: field {k}\n  want {want}\n  got  {got}",
                c.file, c.name
            )),
            None => out.push(format!(
                "{} / {}: field {k} not produced",
                c.file, c.name
            )),
        }
    }
    out
}

fn run(files: &[&str], project: impl Fn(&Case) -> Map<String, Value>) {
    let mut failures = Vec::new();
    let mut n = 0;
    for f in files {
        for c in cases(f) {
            n += 1;
            failures.extend(compare(&c, &project(&c)));
        }
    }
    assert!(n > 0, "no vectors in {files:?}");
    assert!(
        failures.is_empty(),
        "{} of {n} vectors failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn thread_vectors() {
    run(&["thread/thread.json", "thread/depth.json"], |c| {
        let (_, input) = forum_input(c);
        let r = fold(&input).unwrap();
        let p = c.input["query"]["thread"].as_str().unwrap();
        let mut out = Map::new();
        let Some(th) = r.threads.get(p) else {
            out.insert("rootMissing".into(), json!(true));
            for k in ["parent", "depth"] {
                out.insert(k.into(), json!({}));
            }
            for k in ["missing", "orphaned"] {
                out.insert(k.into(), json!([]));
            }
            out.insert("maxDepth".into(), json!(0));
            return out;
        };
        let pl = &th.placement;
        let parent: Map<String, Value> = pl
            .parent
            .iter()
            .map(|(c, par)| {
                let to = match par {
                    Parent::Root => p.to_owned(),
                    Parent::Comment(u) | Parent::Missing(u) => u.clone(),
                };
                (c.clone(), json!(to))
            })
            .collect();
        out.insert("rootMissing".into(), json!(pl.root_missing));
        out.insert("parent".into(), Value::Object(parent));
        out.insert("missing".into(), json!(pl.missing));
        out.insert("depth".into(), json!(pl.depth));
        out.insert("orphaned".into(), json!(pl.orphaned));
        out.insert("maxDepth".into(), json!(pl.max_depth()));
        out
    });
}

#[test]
fn time_vectors() {
    run(&["time/time.json"], |c| {
        let (_, input) = forum_input(c);
        let r = fold(&input).unwrap();
        let times: Map<String, Value> = r
            .items
            .iter()
            .filter_map(|(u, i)| {
                Some((
                    u.clone(),
                    json!({
                        "tRank": i.t_rank?,
                        "futureDated": i.future,
                        "inconsistentTimestamps": i.inconsistent,
                    }),
                ))
            })
            .collect();
        let mut out = Map::new();
        out.insert("tDecl".into(), json!(r.t_decl));
        out.insert("times".into(), Value::Object(times));
        out
    });
}

#[test]
fn time_conversion_vectors() {
    run(&["time/conversions.json"], |c| {
        let us = match c.input["fn"].as_str().unwrap() {
            "us" => time::us(c.input["datetime"].as_str().unwrap()),
            "tidTime" => time::tid_time(c.input["tid"].as_str().unwrap()),
            other => panic!("unknown fn {other}"),
        };
        let mut out = Map::new();
        out.insert("us".into(), json!(us));
        out
    });
}

#[test]
fn digest_vectors() {
    run(&["fold/digest.json"], |c| {
        let (fi, input) = forum_input(c);
        let r = fold(&input).unwrap();
        let pre = digest::preimage(&fi).unwrap();
        let mut out = Map::new();
        out.insert("algs".into(), json!(r.policy.algs_line()));
        out.insert("params".into(), json!(r.policy.params_line()));
        out.insert("newerRulesBanner".into(), json!(r.policy.newer_rules));
        out.insert(
            "digestPreimage".into(),
            json!(String::from_utf8(pre).unwrap()),
        );
        out.insert(
            "inputDigest".into(),
            json!(digest::input_digest(&fi).unwrap()),
        );
        out
    });
}
