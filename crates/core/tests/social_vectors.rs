// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs the social fold vectors that also hold in this build's default namespace
//! (`testdata/vectors/README.md`, "Social fold vectors"): `thread/*.json` (§11 placement and
//! step 6 versions), `time/*.json` (§2, the edited badge), and the `algs`/`params`/`inputDigest`
//! fields of `fold/digest.json` (§1.2), through the full fold ([`fold_view_with`], the one the
//! engine runs) with the runner's validity, projected like every other runner ([`project`]).
//!
//! **Namespace.** The vectors are written in the final namespace (`ns` in each file). With the
//! `final-ns` feature (`cargo test -p lessdugin-core --features final-ns --test social_vectors`)
//! this build's lexicon is that namespace and record validity (rule V1) is computed exactly. In a
//! default (`.temp`) build each record is checked after mapping its NSIDs into this build's
//! namespace, ignoring the rules whose outcome depends on keys hashed from the namespace (G5, G9,
//! the key equalities and RV5): the vectors' deliberately invalid records break other rules.
//! G9 alone is then checked on the original values (`fk(value.forum)` hashes the forum uri as
//! written, so it needs no mapping): the CO5 vectors' roots are invalid by G9 only. The fold
//! itself always runs on the original values with the file's `ns`.
//!
//! **The full fold.** In a `final-ns` build this binary also holds the full fold's runner of
//! every vector file and kind (`social_fold.rs`) and the `Query` contract over every vector
//! input (`social_query.rs`), as modules, so the one command CI and `cargo xtask ci` run at the
//! final namespace (`--features final-ns --test social_vectors`) covers all of them. They stay
//! runnable alone (`--test social_fold`, `--test social_query`).
//!
//! **wasm32.** The same binary also runs on wasm32 under node (`cargo xtask test-wasm`, in both
//! namespaces; `social.md` §1.3 rule 6, `engine-api.md` §3.9): its results equal the native
//! run's, which proves native = wasm for the social vectors. There is no filesystem there, so
//! every social vector file is embedded at compile time from `FILES` (the modules read them
//! through [`vector_text`]), which `wasm_embeds_every_social_vector_file` keeps equal to the
//! directory listing; on wasm32 the full fold's runners exist only as modules of this binary.

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::social::digest;
use lessdugin_core::social::full::{ForumView, Query, fold_view_with, project};
use lessdugin_core::social::input::FoldInput;
use lessdugin_core::social::time;
use lessdugin_lexicon::ns::{NS, to_build_nsid};
use lessdugin_lexicon::{RuleId, keys};
use serde_json::{Map, Value, json};
// Natively this is a libtest test; on wasm32 the same body runs under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[cfg(feature = "final-ns")]
#[path = "social_fold.rs"]
mod social_fold;
#[cfg(feature = "final-ns")]
#[path = "social_query.rs"]
mod social_query;

/// The social fold vector directories (`social.md` §19; `social_fold.rs` runs all of them).
#[cfg(not(target_arch = "wasm32"))]
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

/// `FILES`: every vector file under `testdata/vectors/{DIRS}`, in run order (`DIRS` order, then by
/// name); on wasm32 also `EMBEDDED`, their text.
macro_rules! vector_files {
    ($($rel:literal),* $(,)?) => {
        const FILES: &[&str] = &[$($rel),*];
        #[cfg(target_arch = "wasm32")]
        const EMBEDDED: &[&str] = &[$(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/vectors/", $rel))
        ),*];
    };
}

vector_files![
    "time/conversions.json",
    "time/time.json",
    "modfold/author.json",
    "modfold/forum.json",
    "modfold/participation.json",
    "karma/gates.json",
    "karma/power.json",
    "karma/scores.json",
    "karma/staged.json",
    "karma/tags-large.json",
    "karma/tags.json",
    "reacts/reacts.json",
    "rank/rank.json",
    "visibility/visibility.json",
    "thread/depth.json",
    "thread/thread.json",
    "fold/digest.json",
];

#[cfg(not(target_arch = "wasm32"))]
fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/vectors")
}

/// The text of the vector file `rel` (`dir/name.json`), read from disk.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn vector_text(rel: &str) -> String {
    let path = vectors_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// The text of the vector file `rel` (`dir/name.json`), embedded at compile time.
#[cfg(target_arch = "wasm32")]
pub(crate) fn vector_text(rel: &str) -> String {
    let i = FILES
        .iter()
        .position(|f| *f == rel)
        .unwrap_or_else(|| panic!("{rel} is not in FILES"));
    EMBEDDED[i].to_owned()
}

/// The wasm32 run sees exactly the files the native run reads.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn wasm_embeds_every_social_vector_file() {
    let mut on_disk = Vec::new();
    for d in DIRS {
        let mut names: Vec<String> = std::fs::read_dir(vectors_dir().join(d))
            .unwrap_or_else(|e| panic!("{d}: {e}"))
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();
        on_disk.extend(names.into_iter().map(|n| format!("{d}/{n}")));
    }
    assert_eq!(
        on_disk, FILES,
        "tests/social_vectors.rs FILES (embedded for the wasm32 run) must list every social vector \
         file"
    );
}

fn load(rel: &str) -> Value {
    serde_json::from_str(&vector_text(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
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

/// G9 on the original values: `fk(value.forum)` equals the key's fk segment (`data-model.md` §7;
/// a value without a string `forum` has nothing to compare here).
fn g9(uri: &str, value: &Value) -> bool {
    let Some(forum) = value.get("forum").and_then(Value::as_str) else {
        return true;
    };
    let rkey = uri.rsplit('/').next().unwrap_or_default();
    keys::fk(forum).is_ok_and(|fk| keys::fk_of(rkey) == Some(fk.as_str()))
}

/// `data-model.md` §7 for a vector record in namespace `ns`.
fn validity(ns: &str, uri: &str, value: &Value) -> (bool, Option<String>) {
    let exact = ns == NS;
    if !exact && !g9(uri, value) {
        return (false, Some(RuleId::G9.to_string()));
    }
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

fn forum_input(c: &Case) -> FoldInput {
    FoldInput::from_vector_json(&c.input["fold"]).unwrap()
}

/// The full fold of a case's input under its `query` (`thread` names the placed thread).
fn fold(c: &Case, fi: &FoldInput) -> ForumView {
    let ns = c.ns.clone();
    let query: Query = match c.input.get("query") {
        None | Some(Value::Null) => Query::default(),
        Some(q) => serde_json::from_value(q.clone())
            .unwrap_or_else(|e| panic!("{} / {}: query: {e}", c.file, c.name)),
    };
    fold_view_with(fi, &c.ns, &|u, v| validity(&ns, u, v), &query)
        .unwrap_or_else(|e| panic!("{} / {}: {e}", c.file, c.name))
}

/// `fields` of the fold of `c` ([`project`]).
fn projected(c: &Case, fields: &[&str]) -> Map<String, Value> {
    let fi = forum_input(c);
    let r = fold(c, &fi);
    fields
        .iter()
        .map(|f| {
            let v = project(&r, f).unwrap_or_else(|e| panic!("{} / {}: {e:?}", c.file, c.name));
            ((*f).to_owned(), v)
        })
        .collect()
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
            None => out.push(format!("{} / {}: field {k} not produced", c.file, c.name)),
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
        projected(
            c,
            &[
                "rootMissing",
                "parent",
                "missing",
                "depth",
                "orphaned",
                "maxDepth",
                "versions",
            ],
        )
    });
}

#[test]
fn time_vectors() {
    run(&["time/time.json"], |c| {
        projected(c, &["tDecl", "times", "edited"])
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
        let fi = forum_input(c);
        let r = fold(c, &fi);
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
