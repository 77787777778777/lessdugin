// SPDX-License-Identifier: AGPL-3.0-or-later
//! M2 acceptance (`docs/plan.md` §3 M2): "A 50-member FakePds forum cold-syncs natively in
//! under 5 s."
//!
//! A forum of its owner and 50 approved members (granted with `modAction` access records, as
//! `lessdugin approve` writes them) on a FakePds network served over real HTTP: half of the
//! members post, comment and vote, half only vote (the shape of `sync.md` §14.1's cost model,
//! `a_cold_sync_of_fifty_members_matches_the_cost_model` in `crates/core/tests/sync.rs`). An
//! anonymous reader on a fresh data directory runs `lessdugin --dev sync <forum>`: the native
//! binary, the native host (SQLite state, real HTTP) and a cold cache. The wall time of that
//! process (start-up included) must stay under [`ACCEPTANCE`] (5 s).
//!
//! The time is always recorded: printed, written to `LESSDUGIN_TIMINGS_FILE` (JSON lines) when
//! that is set, and added to the GitHub Actions job summary. An optimised build (`cargo test
//! --release --features devnet`, a step of CI's rust job) always asserts 5 s: that is the
//! acceptance check. A debug build on CI (`CI` set) asserts only [`CI_SLACK`] × 5 s, since
//! `cargo test` builds without optimisation (the workspace crates, the fake PDS included, run
//! at opt-level 0); a time over 5 s there is a warning annotation. `LESSDUGIN_COLD_SYNC_BUDGET_MS` sets the bound
//! explicitly (never below 5 s).

#![allow(clippy::disallowed_methods)] // a real clock for a real process

#[path = "support/fakenet.rs"]
mod fakenet;

use std::cell::Cell;
use std::time::{Duration, Instant};

use fakenet::Net;
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_core::discovery::ForumKey;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::records::{
    AccessAction, Action, Forum, Markdown, Md, Membership, ModAction, Opinion, Post, StrongRef,
};
use lessdugin_lexicon::values::{AccessLevel, MembershipMode};
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::repo::tid_at_ms;
use serde_json::{Value, json};

/// The acceptance bound (`docs/plan.md` M2).
const ACCEPTANCE: Duration = Duration::from_secs(5);
/// The factor on CI.
const CI_SLACK: u32 = 3;
/// Members besides the owner.
const MEMBERS: usize = 50;

/// Writes the forum's records straight into the fake repositories.
struct Seed {
    /// Record times: an hour ago, one millisecond apart.
    base_ms: i64,
    seq: Cell<i64>,
}

impl Seed {
    fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        Self {
            base_ms: i64::try_from(now).unwrap() - 3_600_000,
            seq: Cell::new(0),
        }
    }

    fn ms(&self) -> i64 {
        self.seq.set(self.seq.get() + 1);
        self.base_ms + self.seq.get()
    }

    fn dt(&self) -> Datetime {
        Datetime::from_millis(self.ms()).unwrap()
    }

    fn tid(&self) -> String {
        tid_at_ms(self.ms()).to_string()
    }

    fn put(&self, a: &AccountHandle, coll: &str, rkey: &str, v: &Value) -> String {
        let uri = AtUri::parse(&a.uri(coll, rkey)).unwrap();
        let validity = lessdugin_lexicon::validity::check_uri(&uri, v);
        assert!(validity.first_rule().is_none(), "{uri}: {validity:?}");
        a.put(coll, rkey, v);
        uri.to_string()
    }

    fn forum(&self, owner: &AccountHandle, slug: &str) -> ForumKey {
        let key = ForumKey::new(owner.did(), slug).unwrap();
        let f = Forum::create(slug, self.dt(), MembershipMode::Approval);
        self.put(owner, nsid::FORUM, slug, &serde_json::to_value(&f).unwrap());
        self.join(owner, &key);
        key
    }

    fn join(&self, a: &AccountHandle, f: &ForumKey) {
        let m = Membership::new(f.uri.clone(), self.dt());
        let v = serde_json::to_value(&m).unwrap();
        self.put(a, nsid::MEMBERSHIP, &f.fk, &v);
    }

    fn grant(&self, owner: &AccountHandle, f: &ForumKey, subject: &Did) {
        let action = Action::Access(AccessAction::new(AccessLevel::Member));
        let m = ModAction::new(f.uri.clone(), subject.as_str(), action, self.dt());
        let v = serde_json::to_value(&m).unwrap();
        let am = v["action"].as_object().unwrap().clone();
        let rkey = keys::mod_action_rkey(NS, f.uri.as_str(), None, subject.as_str(), &am).unwrap();
        self.put(owner, nsid::MOD_ACTION, &rkey, &v);
    }

    /// A post; its uri and CID.
    fn post(&self, a: &AccountHandle, f: &ForumKey, n: usize) -> (String, String) {
        let p = Post::article(
            f.uri.clone(),
            format!("Post {n}"),
            "Some body text.",
            self.dt(),
        );
        let rkey = keys::post_rkey(f.uri.as_str(), &self.tid()).unwrap();
        let uri = self.put(a, nsid::POST, &rkey, &serde_json::to_value(&p).unwrap());
        let cid = a.cid(nsid::POST, &rkey).unwrap().to_string();
        (uri, cid)
    }

    fn opinion(&self, a: &AccountHandle, f: &ForumKey, subject: &(String, String)) {
        let mut o = Opinion::new(
            StrongRef::new(AtUri::parse(&subject.0).unwrap(), subject.1.clone()),
            f.uri.clone(),
            self.dt(),
        );
        o.karma = Some("up".into());
        let rkey = keys::opinion_rkey(NS, f.uri.as_str(), &subject.0).unwrap();
        self.put(a, nsid::OPINION, &rkey, &serde_json::to_value(&o).unwrap());
    }

    fn comment(&self, a: &AccountHandle, f: &ForumKey, root: &(String, String)) {
        let body = serde_json::to_value(Md::Known(Markdown::inline("A reply."))).unwrap();
        let v = json!({
            "$type": nsid::COMMENT,
            "root": {"uri": root.0, "cid": root.1},
            "forum": f.uri.as_str(),
            "body": body,
            "createdAt": self.dt().as_str(),
        });
        let rkey = keys::comment_rkey(NS, f.uri.as_str(), &root.0, &self.tid()).unwrap();
        self.put(a, nsid::COMMENT, &rkey, &v);
    }
}

/// Records a timing: stdout, `LESSDUGIN_TIMINGS_FILE` (a JSON line), the GitHub job summary
/// (and a warning annotation there when it is over the acceptance figure).
fn record(name: &str, elapsed: Duration, budget: Duration, detail: &str) {
    let ms = elapsed.as_millis();
    println!(
        "{name}: {ms} ms (acceptance {} ms, asserted below {} ms; {detail})",
        ACCEPTANCE.as_millis(),
        budget.as_millis()
    );
    if std::env::var_os("GITHUB_ACTIONS").is_some() && elapsed >= ACCEPTANCE {
        println!(
            "::warning::{name} took {ms} ms, over the {} ms acceptance figure",
            ACCEPTANCE.as_millis()
        );
    }
    let append = |path: std::ffi::OsString, text: String| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(text.as_bytes());
        }
    };
    if let Some(p) = std::env::var_os("LESSDUGIN_TIMINGS_FILE") {
        let line = json!({
            "name": name,
            "ms": ms,
            "acceptanceMs": ACCEPTANCE.as_millis(),
            "boundMs": budget.as_millis(),
            "detail": detail,
        });
        append(p, format!("{line}\n"));
    }
    if let Some(p) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        append(
            p,
            format!(
                "\n### {name}\n\n{ms} ms (acceptance {} ms, asserted below {} ms; {detail})\n",
                ACCEPTANCE.as_millis(),
                budget.as_millis()
            ),
        );
    }
}

#[test]
fn a_fifty_member_forum_cold_syncs_natively_within_five_seconds() {
    let net = Net::new();
    let owner = net.user("owner.example.com", false);
    let seed = Seed::new();
    let f = seed.forum(&owner.account, "fifty");
    let members: Vec<AccountHandle> = (0..MEMBERS)
        .map(|i| {
            let m = net.net.add_account(&format!("p{i}.example.com"));
            seed.join(&m, &f);
            seed.grant(&owner.account, &f, m.did());
            m
        })
        .collect();
    // half write posts, comments and opinions; half only vote
    let first = seed.post(&members[0], &f, 0);
    let mut posts = 1;
    for (i, m) in members.iter().enumerate() {
        if i % 2 == 0 {
            let p = seed.post(m, &f, i + 1);
            posts += 1;
            seed.comment(m, &f, &first);
            seed.opinion(m, &f, &p);
        } else {
            seed.opinion(m, &f, &first);
        }
    }

    let reader = net.reader(&owner);
    let forum = f.uri.to_string();
    let start = Instant::now();
    let report = reader.json(&["sync", &forum]);
    let elapsed = start.elapsed();

    assert_eq!(report["partial"], false, "{report}");
    assert_eq!(report["reposTotal"], MEMBERS + 1, "{report}");
    assert_eq!(report["reposComplete"], MEMBERS + 1, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    let ci = std::env::var_os("CI").is_some_and(|v| !v.is_empty());
    let budget = std::env::var("LESSDUGIN_COLD_SYNC_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms).max(ACCEPTANCE))
        .unwrap_or(if ci && cfg!(debug_assertions) {
            ACCEPTANCE * CI_SLACK
        } else {
            ACCEPTANCE
        });
    record(
        "cold sync of a 50-member forum (native CLI, FakePds over HTTP)",
        elapsed,
        budget,
        &format!(
            "{} repositories, {} requests, {} bytes; {} build",
            report["reposTotal"],
            report["requests"],
            report["bytes"],
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        ),
    );
    assert!(
        elapsed < budget,
        "the cold sync took {} ms (budget {} ms): {report}",
        elapsed.as_millis(),
        budget.as_millis()
    );

    // what was synced is readable offline: every post, the first one's comments
    let list = reader.json(&["--offline", "ls", &forum, "--sort", "new", "-n", "100"]);
    assert_eq!(list["items"].as_array().unwrap().len(), posts, "{list}");
    let thread = reader.json(&["--offline", "read", &first.0]);
    assert_eq!(
        thread["comments"].as_array().unwrap().len(),
        MEMBERS / 2,
        "{thread}"
    );
}
