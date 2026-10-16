// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs the `fk/1` and `sk/1` vectors (`testdata/vectors/keys/{fk,sk,rkeys}.json`, format in
//! `data-model.md` §3.4). Every vector's named result or error is compared, and so is every
//! intermediate the vector lists (the runner exposes all of them, so the sets must match). The
//! `ns` of each rkey vector is passed explicitly, so both prefixes run in every build.

use std::path::PathBuf;

use lessdugin_lexicon::keys::{self, Derivation, KeyError, RkeyInput};
use serde_json::{Map, Value, json};

fn load(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/vectors/keys")
        .join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn err(e: KeyError) -> Value {
    json!({"error": e.code()})
}

fn vectors(doc: &Value) -> &Vec<Value> {
    doc["vectors"].as_array().unwrap()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str()
        .unwrap_or_else(|| panic!("missing string {k} in {v}"))
}

fn check(file: &str, v: &Value, got: Value) {
    assert_eq!(
        got, v["expected"],
        "{file} vector {:?} (input {})",
        v["name"], v["input"]
    );
}

#[test]
fn fk_vectors() {
    let doc = load("fk.json");
    assert_eq!(doc["behaviour"], "fk/1");
    let vs = vectors(&doc);
    assert!(vs.len() >= 20);
    for v in vs {
        let uri = s(&v["input"], "forumUri");
        let got = match keys::hash(keys::TAG_FK, &[uri]) {
            Ok(d) => {
                let fk = keys::fk(uri).unwrap();
                assert_eq!(fk, keys::b32s(&d, 12).unwrap());
                json!({"fk": fk, "digest": hex(&d)})
            }
            Err(e) => {
                assert_eq!(keys::fk(uri), Err(e));
                err(e)
            }
        };
        check("fk.json", v, got);
    }
}

#[test]
fn sk_vectors() {
    let doc = load("sk.json");
    assert_eq!(doc["behaviour"], "sk/1");
    let vs = vectors(&doc);
    let mut by_fn = std::collections::BTreeMap::<&str, usize>::new();
    for v in vs {
        let i = &v["input"];
        let f = s(i, "fn");
        *by_fn.entry(f).or_default() += 1;
        let parts = |k: &str| -> Vec<String> {
            i[k].as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_owned())
                .collect()
        };
        let got = match f {
            "b32s" => {
                let n = i["n"].as_u64().unwrap() as usize;
                json!({"b32s": keys::b32s(&unhex(s(i, "hex")), n).unwrap()})
            }
            "H" => {
                let ps = parts("parts");
                let refs: Vec<&str> = ps.iter().map(String::as_str).collect();
                match keys::preimage(s(i, "tag"), &refs) {
                    Ok(pre) => {
                        let d = keys::hash(s(i, "tag"), &refs).unwrap();
                        assert_eq!(hex(&d), hex(&lessdugin_atp::cid::sha256(&pre)));
                        json!({"preimage": hex(&pre), "digest": hex(&d)})
                    }
                    Err(e) => err(e),
                }
            }
            "h16" => {
                let ps = parts("parts");
                let name = s(i, "name");
                let mut all: Vec<&str> = vec![name];
                all.extend(ps.iter().map(String::as_str));
                let refs: Vec<&str> = ps.iter().map(String::as_str).collect();
                match keys::hash(keys::TAG_SK, &all) {
                    Ok(d) => json!({"h16": keys::h16(name, &refs).unwrap(), "digest": hex(&d)}),
                    Err(e) => {
                        assert_eq!(keys::h16(name, &refs), Err(e));
                        err(e)
                    }
                }
            }
            other => panic!("unknown fn {other}"),
        };
        check("sk.json", v, got);
    }
    assert!(
        by_fn["b32s"] >= 6 && by_fn["H"] >= 9 && by_fn["h16"] >= 20,
        "{by_fn:?}"
    );
}

fn derivation_json(d: &Derivation) -> Value {
    let mut m = Map::new();
    m.insert("rkey".into(), json!(d.rkey));
    for (k, v) in [
        ("fk", &d.fk),
        ("threadTid", &d.thread_tid),
        ("stid", &d.stid),
        ("h16", &d.h16),
        ("P", &d.p),
        ("scopeEff", &d.scope_eff),
        ("actionKey", &d.action_key),
    ] {
        if let Some(v) = v {
            m.insert(k.into(), json!(v));
        }
    }
    Value::Object(m)
}

#[test]
fn rkey_vectors() {
    let doc = load("rkeys.json");
    assert_eq!(doc["behaviour"], "sk/1");
    let vs = vectors(&doc);
    let mut namespaces = std::collections::BTreeSet::new();
    let mut collections = std::collections::BTreeSet::new();
    for v in vs {
        let i = &v["input"];
        let ns = s(i, "ns");
        namespaces.insert(ns.to_owned());
        let got = match s(i, "fn") {
            "threadTid" => match keys::thread_tid(ns, s(i, "uri")) {
                Ok(t) => json!({"threadTid": t}),
                Err(e) => err(e),
            },
            "stid" => match keys::stid(ns, s(i, "uri")) {
                Ok(t) => json!({"stid": t}),
                Err(e) => err(e),
            },
            "P" => match keys::revision_prefix(ns, s(i, "uri")) {
                Ok(p) => json!({"P": p}),
                Err(e) => err(e),
            },
            "actionKey" => match keys::action_key(ns, i["action"].as_object().unwrap()) {
                Ok(k) => json!({"actionKey": k}),
                Err(e) => err(e),
            },
            "rkey" => {
                let coll = s(i, "collection");
                collections.insert(coll.to_owned());
                let f = |k: &str| i[k].as_str().unwrap_or_default();
                let empty = Map::new();
                let action = i["action"].as_object().unwrap_or(&empty);
                let input = match coll {
                    "forum" => RkeyInput::Forum,
                    "profile" => RkeyInput::Profile,
                    "membership" => RkeyInput::Membership { forum: f("forum") },
                    "post" => RkeyInput::Post {
                        forum: f("forum"),
                        tid: f("tid"),
                    },
                    "wikiRevision" => RkeyInput::WikiRevision {
                        forum: f("forum"),
                        tid: f("tid"),
                    },
                    "sequence" => RkeyInput::Sequence {
                        forum: f("forum"),
                        tid: f("tid"),
                    },
                    "comment" => RkeyInput::Comment {
                        forum: f("forum"),
                        root: f("root"),
                        tid: f("tid"),
                    },
                    "revision" => RkeyInput::Revision {
                        subject: f("subject"),
                        tid: f("tid"),
                    },
                    "opinion" => RkeyInput::Opinion {
                        forum: f("forum"),
                        subject: f("subject"),
                    },
                    "modAction" => RkeyInput::ModAction {
                        forum: f("forum"),
                        scope: i["scope"].as_str(),
                        subject: f("subject"),
                        action,
                    },
                    other => panic!("unknown collection {other}"),
                };
                match keys::derive(ns, &input) {
                    Ok(d) => {
                        // the convenience wrappers agree with derive
                        let wrapped = match input {
                            RkeyInput::Membership { forum } => keys::membership_rkey(forum),
                            RkeyInput::Post { forum, tid } => keys::post_rkey(forum, tid),
                            RkeyInput::WikiRevision { forum, tid } => {
                                keys::wiki_revision_rkey(forum, tid)
                            }
                            RkeyInput::Sequence { forum, tid } => keys::sequence_rkey(forum, tid),
                            RkeyInput::Comment { forum, root, tid } => {
                                keys::comment_rkey(ns, forum, root, tid)
                            }
                            RkeyInput::Revision { subject, tid } => {
                                keys::revision_rkey(ns, subject, tid)
                            }
                            RkeyInput::Opinion { forum, subject } => {
                                keys::opinion_rkey(ns, forum, subject)
                            }
                            RkeyInput::ModAction {
                                forum,
                                scope,
                                subject,
                                action,
                            } => keys::mod_action_rkey(ns, forum, scope, subject, action),
                            RkeyInput::Profile => Ok(keys::PROFILE_RKEY.to_owned()),
                            RkeyInput::Forum => unreachable!(),
                        };
                        assert_eq!(wrapped.as_deref(), Ok(d.rkey.as_str()), "{}", v["name"]);
                        derivation_json(&d)
                    }
                    Err(e) => err(e),
                }
            }
            other => panic!("unknown fn {other}"),
        };
        check("rkeys.json", v, got);
    }
    // the file exercises both prefixes and every collection whose key is derived (a forum's
    // key is its slug)
    assert!(namespaces.contains(lessdugin_lexicon::ns::NS_FINAL));
    assert!(namespaces.contains(lessdugin_lexicon::ns::NS_TEMP));
    for c in lessdugin_lexicon::Collection::ALL {
        if c != lessdugin_lexicon::Collection::Forum {
            assert!(collections.contains(c.short_name()), "{c}");
        }
    }
    assert_eq!(
        keys::derive(lessdugin_lexicon::ns::NS, &RkeyInput::Forum),
        Err(KeyError::Undefined)
    );
}

#[test]
fn grammar_of_every_derived_rkey() {
    use lessdugin_lexicon::Collection;
    // every successfully derived non-raw key matches its collection's grammar
    let doc = load("rkeys.json");
    for v in vectors(&doc) {
        let i = &v["input"];
        if i["fn"] != "rkey" || v.get("raw").is_some() {
            continue;
        }
        let Some(rkey) = v["expected"]["rkey"].as_str() else {
            continue;
        };
        let c = Collection::from_short_name(s(i, "collection")).unwrap();
        assert!(keys::matches_grammar(c, rkey), "{}: {rkey}", v["name"]);
    }
}
