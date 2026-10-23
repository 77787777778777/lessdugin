// SPDX-License-Identifier: AGPL-3.0-or-later
//! The CLI part of the parity scenario (`docs/plan.md` §4.2, M2 text subset) against the local
//! devnet (`packages/e2e/devnet.mjs`, ADR 0006). Runs only with `LESSDUGIN_E2E=1`;
//! `LESSDUGIN_DEVNET_JSON=<file>` reuses a running devnet (a fresh one is better: the forum
//! slug is fixed).
//!
//! alice creates an approval-mode forum with the CLI and approves bob and carol; alice and bob
//! post, carol replies; an anonymous reader lists and reads the forum. With the same parity
//! time (`LESSDUGIN_PARITY_NOW_MS`, `engine-api.md` §2.1), the anonymous reader and alice print
//! byte-identical `read --json` output.

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Devnet {
    info: Value,
    _child: Option<lessdugin_testkit::node_devnet::NodeDevnet>,
}

impl Devnet {
    fn start() -> Option<Self> {
        if std::env::var("LESSDUGIN_E2E").ok().as_deref() != Some("1") {
            eprintln!("skipped: set LESSDUGIN_E2E=1 to run against the devnet");
            return None;
        }
        if let Ok(path) = std::env::var("LESSDUGIN_DEVNET_JSON") {
            let info = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            return Some(Self { info, _child: None });
        }
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        // one devnet per test: the tests of a binary run in parallel
        let (child, info) = lessdugin_testkit::node_devnet::NodeDevnet::start(root, "cli-forum");
        Some(Self {
            info,
            _child: Some(child),
        })
    }

    fn s(&self, path: &[&str]) -> String {
        let mut v = &self.info;
        for p in path {
            v = &v[*p];
        }
        v.as_str()
            .unwrap_or_else(|| panic!("devnet.{}", path.join(".")))
            .to_owned()
    }

    /// `lessdugin --dev` with its own data directory.
    fn cli(&self, data: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
        for (k, _) in std::env::vars() {
            if k.starts_with("LESSDUGIN_") {
                c.env_remove(k);
            }
        }
        c.arg("--dev")
            .env("LESSDUGIN_DATA_DIR", data)
            .env("LESSDUGIN_KEYRING", "off")
            .env("LESSDUGIN_PLC_URL", self.s(&["plc"]))
            .env("LESSDUGIN_PDS_URL", self.s(&["pds"]))
            .env("LESSDUGIN_SESSION", "e2e")
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG");
        c
    }
}

fn run(cmd: &mut Command, stdin: Option<&str>) -> Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("lessdugin");
    {
        let mut si = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).unwrap();
        }
    }
    child.wait_with_output().unwrap()
}

/// A CLI user of the devnet.
struct U<'a> {
    devnet: &'a Devnet,
    dir: tempfile::TempDir,
    name: &'static str,
}

impl<'a> U<'a> {
    fn new(devnet: &'a Devnet, name: &'static str, login: bool) -> Self {
        let u = Self {
            devnet,
            dir: tempfile::tempdir().unwrap(),
            name,
        };
        if login {
            let handle = devnet.s(&["accounts", name, "handle"]);
            let pw = devnet.s(&["accounts", name, "appPassword"]);
            let out = run(
                devnet
                    .cli(u.dir.path())
                    .args(["--json", "login", &handle, "--password-stdin"]),
                Some(&format!("{pw}\n")),
            );
            assert!(
                out.status.success(),
                "login {name}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        u
    }

    fn out(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = self.devnet.cli(self.dir.path());
        for (k, v) in env {
            c.env(k, v);
        }
        run(c.args(args), None)
    }

    fn json_env(&self, args: &[&str], env: &[(&str, &str)]) -> (Value, String) {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let out = self.out(&a, env);
        assert!(
            out.status.success(),
            "{} {args:?} failed ({:?}): {}",
            self.name,
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let s = String::from_utf8(out.stdout).unwrap();
        let v = serde_json::from_str(&s).unwrap_or_else(|e| panic!("{args:?}: {e}: {s}"));
        (v, s)
    }

    fn json(&self, args: &[&str]) -> Value {
        self.json_env(args, &[]).0
    }

    fn file(&self, name: &str, text: &str) -> String {
        let p = self.dir.path().join(name);
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }
}

fn done(t: &Value) -> String {
    assert_eq!(t["state"]["kind"], "done", "{t}");
    t["state"]["uri"].as_str().unwrap().to_owned()
}

#[test]
fn e2e_cli_forum_scenario() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    let alice = U::new(&devnet, "alice", true);
    let bob = U::new(&devnet, "bob", true);
    let carol = U::new(&devnet, "carol", true);
    let anon = U::new(&devnet, "alice", false);
    let bob_did = devnet.s(&["accounts", "bob", "did"]);
    let carol_did = devnet.s(&["accounts", "carol", "did"]);

    // step 1: alice creates an approval-mode forum (the mode is written explicitly)
    let slug = format!("parity{}", std::process::id() % 100_000);
    let forum = done(&alice.json(&["forum", "create", &slug, "--name", "Parity"]));
    let show = alice.json(&["forum", "show"]);
    assert_eq!(show["forum"]["uri"], forum);
    assert_eq!(show["forum"]["membership"], "approval");
    assert_eq!(show["myRole"], "owner");
    // she approves bob and carol, and posts
    let a = alice.json(&["approve", "bob.test", "carol.test"]);
    for w in a["writes"].as_array().unwrap() {
        done(w);
    }
    let f = alice.file(
        "welcome.md",
        "---\ntitle: Welcome\n---\nHello from the *CLI*.\n",
    );
    let welcome = done(&alice.json(&["post", &f, "--yes"]));

    // bob joins and posts; carol replies
    let fref = format!("alice.test/{slug}");
    done(&bob.json(&["join", &fref, "--note", "hi"]));
    let f = bob.file("q.md", "A question from bob.\n");
    let question = done(&bob.json(&["post", &f, "--title", "Bob's question", "--yes"]));
    let reply = done(&carol.json(&["comment", &question, "-m", "Carol's answer."]));
    done(&carol.json(&["comment", &welcome, "-m", "Thanks!"]));

    // the members are synced through the grants
    let m = alice.json(&["members"]);
    let dids: Vec<&str> = m["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["actor"]["did"].as_str().unwrap())
        .collect();
    assert!(
        dids.contains(&bob_did.as_str()) && dids.contains(&carol_did.as_str()),
        "{m}"
    );

    // anonymous reading (`--all`: the fixture forum's default tier is personal, and
    // this test is about content and parity, not the M3 frontpage default)
    let list = anon.json(&["ls", &fref, "--all"]);
    let items = list["items"].as_array().unwrap();
    let uris: Vec<&str> = items.iter().map(|p| p["uri"].as_str().unwrap()).collect();
    assert!(
        uris.contains(&welcome.as_str()) && uris.contains(&question.as_str()),
        "{list}"
    );
    let i = uris.iter().position(|u| *u == question).unwrap() + 1;
    let thread = anon.json(&["read", &i.to_string()]);
    assert_eq!(thread["post"]["post"]["uri"], question);
    assert_eq!(thread["comments"][0]["uri"], reply);
    assert_eq!(thread["comments"][0]["author"]["handle"], "carol.test");
    let out = anon.out(&["read", "c1", "--no-pager"], &[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Carol's answer."));

    // parity: the same fixed time gives byte-identical views on two devices
    let t = (chrono_free_now_ms() + 86_400_000).to_string();
    let env = [("LESSDUGIN_PARITY_NOW_MS", t.as_str())];
    let (_, from_anon) = anon.json_env(&["read", &welcome], &env);
    let (_, from_alice) = alice.json_env(&["read", &welcome], &env);
    assert_eq!(from_anon, from_alice);
    let (_, list_anon) = anon.json_env(&["ls", &fref, "--all", "--sort", "old"], &env);
    let (_, list_carol) = carol.json_env(&["ls", &fref, "--all", "--sort", "old"], &env);
    assert_eq!(list_anon, list_carol);
    // parity mode is refused outside devnet mode
    let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
    let out = run(
        c.env("LESSDUGIN_DATA_DIR", anon.dir.path())
            .env("LESSDUGIN_KEYRING", "off")
            .env("LESSDUGIN_PARITY_NOW_MS", &t)
            .args(["ls", &fref]),
        None,
    );
    assert_eq!(out.status.code(), Some(2));
}

fn chrono_free_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
