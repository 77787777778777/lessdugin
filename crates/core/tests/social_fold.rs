// SPDX-License-Identifier: AGPL-3.0-or-later
//! The single runner of every social fold vector (`testdata/vectors/README.md`, "Social fold
//! vectors"; `social.md` §19) against the full fold ([`fold_view`]).
//!
//! - Files: `testdata/vectors/{time,modfold,karma,reacts,rank,visibility,thread,fold}/*.json`.
//! - Dispatch on the file's `kind`: the fold kinds parse `input.fold` into a `FoldInput`, fold
//!   once with `input.query` (or an empty query), and compare each field `expected` lists with
//!   `project(view, field)` (JSON equality); `conversion` and `power` call their function
//!   directly.
//! - `hotBits` are compared **exactly**: `libm::pow` gives the vectors' `python-math.pow` bits
//!   for every value (README "Floats" lets a runner do so). A mismatch within 1 ulp is still a
//!   failure, labelled as such, to be reported with its inputs rather than absorbed.
//! - A field whose layer is still a stub is **pending**, never passed: a vector is `pass` when
//!   every field matches, `pending` when none mismatches but some is pending, `fail` otherwise.
//!   The test fails on any mismatch, on any input the fold rejects, and on any pending vector of
//!   a file in [`MUST_PASS`]; it prints a per-file pass/pending/fail table and the pending stubs.
//!
//! **Namespace.** The vectors are in the final namespace (`ns`), which every key and CID hashes,
//! so this runner exists only in a `final-ns` build:
//! `cargo test -p lessdugin-core --features final-ns --test social_fold -- --nocapture`.

#![cfg(feature = "final-ns")]

use std::collections::BTreeMap;
use std::path::PathBuf;

use lessdugin_core::social::full::karma;
use lessdugin_core::social::full::{Pending, ProjectError, Query, fold_view, project_fold};
use lessdugin_core::social::input::FoldInput;
use lessdugin_core::social::time;
use lessdugin_lexicon::ns::NS;
use serde_json::{Value, json};

/// The vector directories of `social.md` §19.
const DIRS: [&str; 8] = [
    "time",
    "modfold",
    "karma",
    "reacts",
    "rank",
    "visibility",
    "thread",
    "fold",
];

/// Files whose every vector must pass (no pending). Add a file when its layers land.
const MUST_PASS: &[&str] = &[
    "fold/digest.json",
    "karma/gates.json",
    "karma/power.json",
    "karma/scores.json",
    "karma/staged.json",
    "karma/tags.json",
    "modfold/author.json",
    "modfold/forum.json",
    "modfold/participation.json",
    "rank/rank.json",
    "reacts/reacts.json",
    "thread/depth.json",
    "thread/thread.json",
    "time/conversions.json",
    "time/time.json",
    "visibility/visibility.json",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/vectors")
}

fn files() -> Vec<String> {
    let mut out = Vec::new();
    for d in DIRS {
        let dir = root().join(d);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{dir:?}: {e}"))
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();
        out.extend(names.into_iter().map(|n| format!("{d}/{n}")));
    }
    out
}

/// The outcome of one vector.
enum Outcome {
    Pass,
    Pending(Vec<(String, Pending)>),
    Fail(Vec<String>),
}

/// `hotBits` within 1 ulp (README "Floats"): same-sign bit patterns differing by at most 1. Only
/// labels a failure: the comparison is exact.
fn hot_bits_close(want: &Value, got: &Value) -> bool {
    let (Some(w), Some(g)) = (want.as_object(), got.as_object()) else {
        return false;
    };
    let bits = |v: &Value| v.as_str().and_then(|s| u64::from_str_radix(s, 16).ok());
    w.len() == g.len()
        && w.iter()
            .all(|(k, wv)| match (bits(wv), g.get(k).and_then(bits)) {
                (Some(a), Some(b)) => (a >> 63) == (b >> 63) && a.abs_diff(b) <= 1,
                _ => false,
            })
}

fn compare(want: &Value, got: &Value) -> bool {
    got == want
}

fn algorithm_ids_match(fold: &Value, view: &lessdugin_core::social::full::ForumView) -> bool {
    let Some(ids) = fold.get("algorithmIds") else {
        return true;
    };
    let p = &view.policy;
    *ids == json!({
        "moderation": p.moderation.as_str(),
        "karma": p.karma.as_str(),
        "ranking": p.ranking.as_str(),
        "reactPalette": p.react_palette.as_str(),
    })
}

fn run_vector(kind: &str, input: &Value, expected: &serde_json::Map<String, Value>) -> Outcome {
    let mut fails = Vec::new();
    let mut pending = Vec::new();
    let mut check = |field: &str, got: Result<Value, ProjectError>| match got {
        Ok(got) => {
            let want = &expected[field];
            if !compare(want, &got) {
                let ulp = if field == "hotBits" && hot_bits_close(want, &got) {
                    " (hotBits within 1 ulp of python-math.pow: report the inputs)"
                } else {
                    ""
                };
                fails.push(format!(
                    "field {field}{ulp}\n    want {want}\n    got  {got}"
                ));
            }
        }
        Err(ProjectError::Pending(p)) => pending.push((field.to_owned(), p)),
        Err(ProjectError::UnknownField(f)) => fails.push(format!("unknown field {f}")),
        Err(ProjectError::NeedsInput(e)) => fails.push(format!("field {field}: {e}")),
    };
    match kind {
        "conversion" => {
            let us = match input["fn"].as_str() {
                Some("us") => time::us(input["datetime"].as_str().unwrap_or_default()),
                Some("tidTime") => time::tid_time(input["tid"].as_str().unwrap_or_default()),
                other => return Outcome::Fail(vec![format!("unknown conversion fn {other:?}")]),
            };
            for field in expected.keys() {
                let got = match field.as_str() {
                    "us" => Ok(json!(us)),
                    f => Err(ProjectError::UnknownField(f.to_owned())),
                };
                check(field, got);
            }
        }
        "power" => {
            let (Some(k), Some(x)) = (input["k"].as_i64(), input["x"].as_str()) else {
                return Outcome::Fail(vec!["power input".into()]);
            };
            for field in expected.keys() {
                let got = match field.as_str() {
                    "power" => match karma::power(k, x) {
                        Some(p) => Ok(json!(p)),
                        None => return Outcome::Fail(vec![format!("unknown power value {x}")]),
                    },
                    f => Err(ProjectError::UnknownField(f.to_owned())),
                };
                check(field, got);
            }
        }
        "time" | "modfold" | "thread" | "digest" | "visibility" | "rank" | "karma" | "reacts"
        | "tags" => {
            let fold = &input["fold"];
            let fi = match FoldInput::from_vector_json(fold) {
                Ok(fi) => fi,
                Err(e) => return Outcome::Fail(vec![format!("input error: {e}")]),
            };
            let query: Query = match input.get("query") {
                None | Some(Value::Null) => Query::default(),
                Some(q) => match serde_json::from_value(q.clone()) {
                    Ok(q) => q,
                    Err(e) => return Outcome::Fail(vec![format!("query error: {e}")]),
                },
            };
            let view = match fold_view(&fi, &query) {
                Ok(v) => v,
                Err(e) => return Outcome::Fail(vec![format!("fold rejected the input: {e}")]),
            };
            if !algorithm_ids_match(fold, &view) {
                return Outcome::Fail(vec!["input error: algorithmIds differ from decl".into()]);
            }
            for field in expected.keys() {
                check(field, project_fold(&fi, &view, field));
            }
        }
        other => return Outcome::Fail(vec![format!("unknown kind {other}")]),
    }
    if !fails.is_empty() {
        Outcome::Fail(fails)
    } else if !pending.is_empty() {
        Outcome::Pending(pending)
    } else {
        Outcome::Pass
    }
}

#[derive(Default)]
struct Tally {
    pass: usize,
    pending: usize,
    fail: usize,
}

#[test]
fn social_fold_vectors() {
    let mut table: Vec<(String, String, Tally)> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    // stub → (file → pending vector count), and pending fields per stub
    let mut stubs: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut stub_fields: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    let mut total = 0;
    for rel in files() {
        let text = std::fs::read_to_string(root().join(&rel)).unwrap();
        let f: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let kind = f["kind"].as_str().unwrap_or_default().to_owned();
        let ns = f["ns"].as_str().unwrap_or_default();
        assert_eq!(ns, NS, "{rel}: vectors in namespace {ns}, build in {NS}");
        let mut t = Tally::default();
        for v in f["vectors"].as_array().unwrap() {
            total += 1;
            let name = v["name"].as_str().unwrap_or_default();
            let expected = v["expected"].as_object().cloned().unwrap_or_default();
            match run_vector(&kind, &v["input"], &expected) {
                Outcome::Pass => t.pass += 1,
                Outcome::Pending(ps) => {
                    t.pending += 1;
                    if MUST_PASS.contains(&rel.as_str()) {
                        failures.push(format!(
                            "{rel} / {name}: pending in a must-pass file: {}",
                            ps.iter()
                                .map(|(f, p)| format!("{f} ← {p}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    let mut seen = std::collections::BTreeSet::new();
                    for (field, p) in ps {
                        let key = p.to_string();
                        stub_fields.entry(key.clone()).or_default().insert(field);
                        if seen.insert(key.clone()) {
                            *stubs
                                .entry(key)
                                .or_default()
                                .entry(rel.clone())
                                .or_default() += 1;
                        }
                    }
                }
                Outcome::Fail(fs) => {
                    t.fail += 1;
                    for f in fs {
                        failures.push(format!("{rel} / {name}: {f}"));
                    }
                }
            }
        }
        table.push((rel, kind, t));
    }

    let mut report = String::from("\nsocial fold vectors (full fold)\n");
    report.push_str(&format!(
        "{:<30} {:<11} {:>5} {:>5} {:>8} {:>5}\n",
        "file", "kind", "total", "pass", "pending", "fail"
    ));
    let (mut p, mut q, mut r) = (0, 0, 0);
    for (rel, kind, t) in &table {
        report.push_str(&format!(
            "{:<30} {:<11} {:>5} {:>5} {:>8} {:>5}\n",
            rel,
            kind,
            t.pass + t.pending + t.fail,
            t.pass,
            t.pending,
            t.fail
        ));
        p += t.pass;
        q += t.pending;
        r += t.fail;
    }
    report.push_str(&format!(
        "{:<30} {:<11} {:>5} {:>5} {:>8} {:>5}\n",
        "all", "", total, p, q, r
    ));
    if !stubs.is_empty() {
        report.push_str("\npending, by stub (vectors per file; fields):\n");
        for (stub, by_file) in &stubs {
            let files: Vec<String> = by_file.iter().map(|(f, n)| format!("{f} {n}")).collect();
            let fields: Vec<&str> = stub_fields[stub].iter().map(String::as_str).collect();
            report.push_str(&format!(
                "  {stub}\n    {}\n    fields: {}\n",
                files.join(", "),
                fields.join(", ")
            ));
        }
    }
    println!("{report}");
    assert!(total > 0, "no vectors found");
    assert!(
        failures.is_empty(),
        "{report}\n{} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
