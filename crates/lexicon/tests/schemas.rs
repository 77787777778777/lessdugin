// SPDX-License-Identifier: MIT OR Apache-2.0
//! Schema registry: the embedded list matches `lexicons/`, every document compiles and resolves,
//! the inventory matches `data-model.md` §1.2, and a maximal sample of every definition passes
//! the validator.

mod common;

// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
use common::repo_path;
use common::{nsid, sample_def};
use lessdugin_atp::syntax::RecordKeyType;
use lessdugin_lexicon::ns::{NS, NS_FINAL, to_build_nsid};
use lessdugin_lexicon::schemas::{Def, EXTERNAL, LESSDUGIN, Registry, TypeDef, registry};
use lessdugin_lexicon::validate::{Options, Validator};

#[cfg(not(target_arch = "wasm32"))]
fn walk(dir: &Path, base: &Path, out: &mut BTreeSet<String>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.insert(
                p.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

// reads `lexicons/` from disk, so native only
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn embedded_list_matches_the_directory() {
    let base = repo_path("lexicons");
    let mut on_disk = BTreeSet::new();
    walk(&base, &base, &mut on_disk);
    let embedded: BTreeSet<String> = LESSDUGIN.iter().map(|e| e.path.to_owned()).collect();
    assert_eq!(embedded.len(), LESSDUGIN.len(), "duplicate entries");
    assert_eq!(
        embedded, on_disk,
        "schemas::LESSDUGIN must list exactly the files under lexicons/"
    );
    // the embedded text is the file text
    for e in LESSDUGIN {
        assert_eq!(
            std::fs::read_to_string(base.join(e.path)).unwrap(),
            e.json,
            "{}",
            e.path
        );
    }
    let ext_base = Path::new(env!("CARGO_MANIFEST_DIR")).join("lexicons");
    let mut ext = BTreeSet::new();
    walk(&ext_base, &ext_base, &mut ext);
    let embedded_ext: BTreeSet<String> = EXTERNAL.iter().map(|e| e.path.to_owned()).collect();
    assert_eq!(embedded_ext, ext);
}

#[test]
fn ids_match_paths_and_map_to_the_build_prefix() {
    let reg = registry();
    assert!(reg.errors().is_empty(), "{:?}", reg.errors());
    for e in LESSDUGIN.iter().chain(EXTERNAL) {
        let file_id = e.path.trim_end_matches(".json").replace('/', ".");
        let build_id = to_build_nsid(&file_id).unwrap_or_else(|| file_id.clone());
        let doc = reg
            .doc(&build_id)
            .unwrap_or_else(|| panic!("{build_id} not in the registry"));
        assert_eq!(doc.file_id, file_id);
        assert_eq!(doc.id, build_id);
        if LESSDUGIN.iter().any(|x| x.path == e.path) {
            assert!(file_id.starts_with(NS_FINAL), "{file_id}");
            assert!(doc.id.starts_with(NS), "{}", doc.id);
        }
    }
    assert!(
        reg.unresolved_refs().is_empty(),
        "{:?}",
        reg.unresolved_refs()
    );
    assert_eq!(reg.docs().count(), LESSDUGIN.len() + EXTERNAL.len());
}

#[test]
fn inventory_matches_data_model_1_2() {
    let reg = registry();
    let mut records: Vec<&str> = reg.record_nsids();
    records.sort_unstable();
    let mut want: Vec<String> = [
        "comment",
        "forum",
        "membership",
        "modAction",
        "opinion",
        "post",
        "profile",
        "revision",
        "sequence",
        "wikiRevision",
    ]
    .iter()
    .map(|s| nsid(s))
    .collect();
    want.sort();
    assert_eq!(records, want);
    for r in &want {
        let rd = reg.record(r).unwrap();
        let expected = if r.ends_with(".profile") {
            RecordKeyType::Literal("self".into())
        } else {
            RecordKeyType::Any
        };
        assert_eq!(rd.key, expected, "{r}");
    }
    assert!(matches!(
        reg.def(&nsid("markdown")),
        Some(Def::Type(TypeDef::Object(_)))
    ));
    for p in ["authFull", "authInteract"] {
        assert!(
            matches!(reg.def(&nsid(p)), Some(Def::PermissionSet(_))),
            "{p}"
        );
    }
    let queries = reg
        .docs()
        .filter(|d| d.id.starts_with(&nsid("index.")))
        .filter(|d| matches!(d.defs.get("main"), Some(Def::Query(_))))
        .count();
    assert_eq!(queries, 15, "§11.3 lists 15 indexer methods");
    // reserved group names are never record names (§1.5)
    for r in reg.record_nsids() {
        let short = r.strip_prefix(&format!("{NS}.")).unwrap();
        assert!(!lessdugin_lexicon::reserved::is_reserved_collection_name(
            short
        ));
    }
}

/// Every definition reference of the registry that is a data type or a record.
fn data_defs(reg: &Registry) -> Vec<String> {
    let mut out = Vec::new();
    for d in reg.docs() {
        for (name, def) in &d.defs {
            if matches!(def, Def::Type(_) | Def::Record(_)) {
                out.push(format!("{}#{name}", d.id));
            }
        }
    }
    out
}

#[test]
fn every_definition_accepts_its_maximal_sample() {
    let reg = registry();
    let v = Validator::new(reg, Options::default());
    let defs = data_defs(reg);
    assert!(defs.len() >= 59, "{}", defs.len());
    for r in defs {
        let sample = sample_def(reg, &r, 0);
        let report = match reg.def(&r) {
            Some(Def::Record(_)) => v.record(r.strip_suffix("#main").unwrap(), None, &sample),
            _ => v.def(&r, &sample),
        };
        assert!(
            report.is_valid(),
            "{r}: {:?}\n{sample:#}",
            report.violations
        );
    }
}

#[test]
fn every_query_output_accepts_its_sample() {
    let reg = registry();
    let v = Validator::new(reg, Options::default());
    for d in reg.docs() {
        let Some(Def::Query(q)) = d.defs.get("main") else {
            continue;
        };
        let Some(schema) = q.output.as_ref().and_then(|o| o.schema.as_ref()) else {
            assert!(
                d.id.ends_with("getRecordProofs"),
                "{} has no JSON output",
                d.id
            );
            continue;
        };
        let sample = common::sample(reg, schema, 0);
        let r = v.output(&d.id, &sample);
        assert!(r.is_valid(), "{}: {:?}", d.id, r.violations);
    }
}

/// Lints over the lessdugin lexicons (`data-model.md` §2.1).
#[test]
fn lexicon_conventions() {
    let reg = registry();
    let mut stack: Vec<(String, &TypeDef)> = Vec::new();
    for d in reg.docs().filter(|d| d.id.starts_with(NS)) {
        for (name, def) in &d.defs {
            let at = format!("{}#{name}", d.id);
            match def {
                Def::Type(t) => stack.push((at, t)),
                Def::Record(r) => stack.extend(
                    r.record
                        .properties
                        .iter()
                        .map(|(n, t)| (format!("{at}.{n}"), t)),
                ),
                Def::Query(q) => {
                    if let Some(p) = &q.parameters {
                        stack.extend(p.properties.iter().map(|(n, t)| (format!("{at}?{n}"), t)));
                    }
                    if let Some(s) = q.output.as_ref().and_then(|o| o.schema.as_ref()) {
                        stack.push((format!("{at}:output"), s));
                    }
                }
                _ => {}
            }
        }
    }
    let mut strings = 0;
    while let Some((at, t)) = stack.pop() {
        match t {
            // unions are always open
            TypeDef::Union(u) => assert!(!u.closed, "{at}: closed union"),
            // kv strings are at most 64 bytes; every plain string has a byte limit
            TypeDef::String(s) => {
                strings += 1;
                if !s.known_values.is_empty() {
                    assert!(
                        s.max_length.is_some_and(|m| m <= 64),
                        "{at}: kv without maxLength ≤ 64"
                    );
                }
                if s.format.is_none() {
                    assert!(s.max_length.is_some(), "{at}: string without maxLength");
                }
                if let (Some(g), Some(b)) = (s.max_graphemes, s.max_length) {
                    assert_eq!(b, g * 10, "{at}: s(G/B) uses B = 10·G");
                }
            }
            TypeDef::Array(a) => {
                assert!(a.max_length.is_some(), "{at}: array without maxLength");
                stack.push((format!("{at}[]"), &a.items));
            }
            TypeDef::Integer(i) => {
                assert!(
                    i.minimum.is_some() && i.maximum.is_some(),
                    "{at}: unbounded integer"
                );
            }
            TypeDef::Object(o) => {
                stack.extend(o.properties.iter().map(|(n, t)| (format!("{at}.{n}"), t)));
            }
            _ => {}
        }
    }
    assert!(strings > 100);
}

#[test]
fn spot_check_limits_against_data_model() {
    // a few limits copied from data-model.md §4–§6, to catch drift between spec and lexicons
    let reg = registry();
    let prop = |coll: &str, name: &str| {
        let Some(Def::Record(r)) = reg.def(&nsid(coll)) else {
            panic!("{coll}");
        };
        r.record.property(name).unwrap().clone()
    };
    let TypeDef::String(s) = prop("forum", "name") else {
        panic!()
    };
    assert_eq!((s.max_graphemes, s.max_length), (Some(100), Some(1000)));
    let TypeDef::String(s) = prop("forum", "membership") else {
        panic!()
    };
    assert_eq!(s.default.as_deref(), Some("approval"));
    let TypeDef::Array(a) = prop("opinion", "reacts") else {
        panic!()
    };
    assert_eq!(a.max_length, Some(32));
    let TypeDef::Array(a) = prop("sequence", "chapters") else {
        panic!()
    };
    assert_eq!(a.max_length, Some(50));
    let TypeDef::String(s) = prop("modAction", "subject") else {
        panic!()
    };
    assert_eq!(s.max_length, Some(3000));
    let Some(Def::Type(TypeDef::Object(md))) = reg.def(&nsid("markdown")) else {
        panic!()
    };
    let Some(TypeDef::String(text)) = md.property("text") else {
        panic!()
    };
    assert_eq!(text.max_length, Some(100_000));
    let Some(TypeDef::Blob(b)) = md.property("textBlob") else {
        panic!()
    };
    assert_eq!(b.max_size, Some(1_000_000));
    assert_eq!(b.accept.as_deref(), Some(&["text/*".to_owned()][..]));
}

#[test]
fn records_keep_their_final_ids_in_files() {
    // the files use the final prefix; the registry never contains the other prefix's records
    let reg = registry();
    let other = if NS == NS_FINAL {
        lessdugin_lexicon::ns::NS_TEMP
    } else {
        NS_FINAL
    };
    for c in lessdugin_lexicon::Collection::ALL {
        assert!(reg.record(&c.nsid_in(other)).is_none(), "{c}");
        assert!(reg.record(c.nsid()).is_some(), "{c}");
    }
}
