// SPDX-License-Identifier: AGPL-3.0-or-later
//! The durable outbox against the fake network (`data-model.md` §8, `sync.md` §7, ADR 0005).

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::block_on;
use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_atp::tid::Tid;
use lessdugin_core::dto::{WriteState, WriteTicket};
use lessdugin_core::engine::ErrorCode;
use lessdugin_core::host::{KvOp, KvStore};
use lessdugin_core::outbox::model::{self, OUTBOX_ROW_VERSION};
use lessdugin_core::outbox::{
    BudgetRow, ContentOpts, EditOpts, Intent, NewWrite, Op, OpinionStep, Outbox, PendingWrite,
    Session, Step, WriteStage, records,
};
use lessdugin_core::store::records::RecordStore;
use lessdugin_core::store::row;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::{Action, HideAction, ModAction, Post};
use lessdugin_testkit::fake_pds::{AccountHandle, Fault};
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::{Value, json};

const APPLY: &str = "com.atproto.repo.applyWrites";
const PUT: &str = "com.atproto.repo.putRecord";
const CREATE: &str = "com.atproto.repo.createRecord";
const DELETE: &str = "com.atproto.repo.deleteRecord";
const HEAD: &str = "com.atproto.sync.getLatestCommit";

struct World {
    host: MemHost,
    net: FakeNet,
    alice: AccountHandle,
    bob: AccountHandle,
    session: Session,
    ob: Outbox<MemHost>,
}

fn world() -> World {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let alice = net.add_account("alice.example.com");
    let bob = net.add_account("bob.example.com");
    let session = Session::parse(alice.did().as_str(), &alice.pds_base()).unwrap();
    let ob = Outbox::new(Arc::new(host.clone()));
    World {
        host,
        net,
        alice,
        bob,
        session,
        ob,
    }
}

impl World {
    fn forum(&self) -> AtUri {
        AtUri::parse(&format!(
            "at://{}/{}/rationality",
            self.bob.did(),
            nsid::FORUM
        ))
        .unwrap()
    }

    fn now(&self) -> Datetime {
        Datetime::from_millis(self.host.now()).unwrap()
    }

    fn post(&self, title: &str) -> Post {
        Post::article(self.forum(), title, "body", self.now())
    }

    fn fk(&self) -> String {
        keys::fk(self.forum().as_str()).unwrap()
    }

    fn userpds(&self, nsid: &str) -> String {
        format!("userpds://{}/xrpc/{nsid}", self.alice.did())
    }

    /// Bodies of the write requests to `nsid` this host sent.
    fn bodies(&self, nsid: &str) -> Vec<Value> {
        self.host
            .requests_to(&self.userpds(nsid))
            .into_iter()
            .map(|r| serde_json::from_slice(r.body.as_deref().unwrap()).unwrap())
            .collect()
    }

    fn run(&self) {
        block_on(self.ob.run_until_idle(&self.session)).unwrap();
    }

    fn ticket(&self, t: &WriteTicket) -> WriteTicket {
        block_on(self.ob.ticket(&t.id)).unwrap().unwrap()
    }

    /// Publishes a post and runs the outbox; `(uri, cid)`.
    fn seed_post(&self, title: &str) -> (String, String) {
        let t = block_on(self.ob.create_post(
            &self.session,
            &self.post(title),
            ContentOpts::default(),
        ))
        .unwrap();
        self.run();
        done(&self.ticket(&t))
    }

    fn cached(&self, uri: &str) -> bool {
        let u = AtUri::parse(uri).unwrap();
        block_on(RecordStore::new(self.host.cache_kv()).get(&u))
            .unwrap()
            .is_some()
    }
}

fn done(t: &WriteTicket) -> (String, String) {
    match &t.state {
        WriteState::Done { uri, cid } => (uri.clone(), cid.clone().unwrap_or_default()),
        other => panic!("write {} is {other:?}", t.id),
    }
}

fn failed_code(t: &WriteTicket) -> ErrorCode {
    match &t.state {
        WriteState::Failed { error } => error.code,
        other => panic!("write {} is {other:?}", t.id),
    }
}

fn rkey_of(uri: &str) -> String {
    uri.rsplit('/').next().unwrap().to_owned()
}

fn tid(ms_ago: u64, n: u64) -> String {
    let micros = (lessdugin_testkit::memhost::DEFAULT_NOW_MS as u64 - ms_ago) * 1000 + n;
    Tid::from_parts(micros, 7).unwrap().as_str().to_owned()
}

// ---------------------------------------------------------------------------------------------
// Persistence, first content, crash safety

#[test]
fn first_content_includes_membership_only_when_absent() {
    let w = world();
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    // persisted before any network call
    assert_eq!(
        t.state,
        WriteState::Pending {
            not_before_ms: None,
            held: false,
            throttled: false
        }
    );
    assert_eq!(w.net.count(APPLY), 0);
    let stored = block_on(w.ob.get(&t.id)).unwrap().unwrap();
    assert_eq!(stored.intent, Intent::FirstContent);
    assert_eq!(stored.state, WriteStage::Queued);

    let heads_before = w.net.count(HEAD);
    w.run();
    let (uri, cid) = done(&w.ticket(&t));
    assert_eq!(
        w.alice.cid(nsid::POST, &rkey_of(&uri)).unwrap().to_string(),
        cid
    );
    assert!(w.alice.get(nsid::MEMBERSHIP, &w.fk()).is_some());
    let bodies = w.bodies(APPLY);
    assert_eq!(bodies.len(), 1);
    let writes = bodies[0]["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 2, "membership + post");
    assert_eq!(writes[0]["collection"], nsid::MEMBERSHIP);
    assert_eq!(writes[1]["collection"], nsid::POST);
    assert!(bodies[0]["swapCommit"].is_string());
    assert!(
        w.net.count(HEAD) > heads_before,
        "a fresh head per applyWrites"
    );
    // applied optimistically to the record store
    assert!(w.cached(&uri));
    let membership = format!("at://{}/{}/{}", w.alice.did(), nsid::MEMBERSHIP, w.fk());
    assert!(w.cached(&membership));

    // the store now shows the membership: a plain createRecord
    let t2 =
        block_on(w.ob.create_post(&w.session, &w.post("two"), ContentOpts::default())).unwrap();
    w.run();
    done(&w.ticket(&t2));
    assert_eq!(w.net.count(CREATE), 1);
    assert_eq!(w.net.count(APPLY), 1);
}

#[test]
fn first_content_skips_a_membership_the_pds_has() {
    let w = world();
    // made on another device: the PDS has it, the local store does not
    let m = records::membership(&w.forum(), None, &w.now()).unwrap();
    w.alice.put(nsid::MEMBERSHIP, &m.rkey, &m.value);
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    w.run();
    done(&w.ticket(&t));
    let bodies = w.bodies(APPLY);
    assert_eq!(bodies[0]["writes"].as_array().unwrap().len(), 1);
    assert_eq!(bodies[0]["writes"][0]["collection"], nsid::POST);
}

#[test]
fn first_content_500_retries_once_without_the_membership() {
    let w = world();
    let calls = Arc::new(AtomicU32::new(0));
    {
        let (net, alice, calls) = (w.net.clone(), w.alice.clone(), Arc::clone(&calls));
        let m = records::membership(&w.forum(), None, &w.now()).unwrap();
        w.host.on_post(&w.userpds(APPLY), move |req| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                // another device joined meanwhile: the batch's membership create fails the
                // whole batch with a 500 (ADR 0005)
                alice.put(nsid::MEMBERSHIP, &m.rkey, &m.value);
                return Ok(lessdugin_testkit::resp::xrpc_error(
                    500,
                    "InternalServerError",
                    "Internal Server Error",
                ));
            }
            net.request(req, "default")
        });
    }
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    w.run();
    let (uri, _) = done(&w.ticket(&t));
    assert!(w.alice.get(nsid::POST, &rkey_of(&uri)).is_some());
    let bodies = w.bodies(APPLY);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["writes"].as_array().unwrap().len(), 2);
    assert_eq!(bodies[1]["writes"].as_array().unwrap().len(), 1);
    assert_eq!(bodies[1]["writes"][0]["collection"], nsid::POST);
    assert_ne!(
        bodies[0]["swapCommit"], bodies[1]["swapCommit"],
        "re-read head"
    );
}

#[test]
fn ambiguous_500_backs_off_then_completes() {
    let w = world();
    w.net.inject(Fault::status(500).on_nsid(APPLY).once());
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    let r = block_on(w.ob.run_due(&w.session)).unwrap();
    assert_eq!(r.throttled, 1);
    let pending = w.ticket(&t);
    let WriteState::Pending {
        throttled: true,
        not_before_ms: Some(at),
        ..
    } = pending.state
    else {
        panic!("{pending:?}")
    };
    assert!(at > w.host.now());
    let stored = block_on(w.ob.get(&t.id)).unwrap().unwrap();
    assert!(stored.sent);
    assert!(stored.error.unwrap().retryable);
    w.run();
    done(&w.ticket(&t));
    assert_eq!(w.net.count(APPLY), 2);
}

/// Runs the outbox until a route panics (a crash), returning the panic.
fn crash(w: &World) {
    let ob = w.ob.clone();
    let s = w.session.clone();
    let r = catch_unwind(AssertUnwindSafe(|| block_on(ob.run_until_idle(&s))));
    assert!(r.is_err(), "the simulated crash did not happen");
}

/// A new process over the same stores, with the network routes back.
fn restart(w: &World) -> Outbox<MemHost> {
    w.host.clear_routes();
    w.net.install(&w.host);
    Outbox::new(Arc::new(w.host.clone()))
}

#[test]
fn crash_after_send_resumes_without_resending() {
    let w = world();
    {
        let net = w.net.clone();
        w.host.on_post(&w.userpds(APPLY), move |req| {
            let _ = net.request(req, "default");
            panic!("simulated crash after the PDS applied the write");
        });
    }
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    crash(&w);
    assert_eq!(w.net.count(APPLY), 1);
    let stored = block_on(w.ob.get(&t.id)).unwrap().unwrap();
    assert_eq!(
        stored.state,
        WriteStage::Sending,
        "persisted before the send"
    );
    assert_eq!(stored.attempts, 1);

    let ob = restart(&w);
    block_on(ob.run_until_idle(&w.session)).unwrap();
    let t = block_on(ob.ticket(&t.id)).unwrap().unwrap();
    let (uri, cid) = done(&t);
    assert_eq!(
        w.net.count(APPLY),
        1,
        "recognised by its CID, not sent again"
    );
    assert_eq!(
        w.alice.cid(nsid::POST, &rkey_of(&uri)).unwrap().to_string(),
        cid
    );
    assert!(w.cached(&uri));
}

#[test]
fn crash_before_send_resends_after_restart() {
    let w = world();
    w.host.on_post(&w.userpds(APPLY), move |_| {
        panic!("simulated crash before the request left");
    });
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    crash(&w);
    assert_eq!(w.net.count(APPLY), 0);
    let ob = restart(&w);
    let r = block_on(ob.run_until_idle(&w.session)).unwrap();
    assert_eq!(r.done, 1);
    let t = block_on(ob.ticket(&t.id)).unwrap().unwrap();
    let (uri, _) = done(&t);
    assert_eq!(w.net.count(APPLY), 1);
    assert!(w.alice.get(nsid::POST, &rkey_of(&uri)).is_some());
}

// ---------------------------------------------------------------------------------------------
// Edit protocol

#[test]
fn edit_retries_invalid_swap_with_jitter() {
    let w = world();
    let (uri, base) = w.seed_post("one");
    let old = w.alice.get(nsid::POST, &rkey_of(&uri)).unwrap();
    w.host.clear_requests();
    w.net.inject(
        Fault::error(400, "InvalidSwap", "Commit was at bafyrei")
            .on_nsid(APPLY)
            .once(),
    );
    let mut new = old.clone();
    new["title"] = json!("one, edited");
    let sleeps = w.host.sleeps().len();
    let heads = w.net.count(HEAD);
    let t = block_on(w.ob.edit(
        &w.session,
        &uri,
        &new,
        &base,
        EditOpts {
            summary: Some("typo".into()),
            ..EditOpts::default()
        },
    ))
    .unwrap();
    w.run();
    let (_, cid) = done(&w.ticket(&t));
    assert_eq!(
        w.alice.cid(nsid::POST, &rkey_of(&uri)).unwrap().to_string(),
        cid
    );
    assert_eq!(
        w.alice.get(nsid::POST, &rkey_of(&uri)).unwrap()["title"],
        "one, edited"
    );
    // two attempts, each after a fresh head, with one jittered wait between them
    let bodies = w.bodies(APPLY);
    assert_eq!(bodies.len(), 2);
    assert!(w.net.count(HEAD) >= heads + 2);
    let waits = &w.host.sleeps()[sleeps..];
    assert_eq!(waits.len(), 1);
    assert!((200..=800).contains(&waits[0]), "{waits:?}");
    // the revision snapshots the old value verbatim
    let writes = bodies[1]["writes"].as_array().unwrap();
    assert_eq!(writes[0]["collection"], nsid::REVISION);
    assert_eq!(writes[1]["$type"], "com.atproto.repo.applyWrites#update");
    let rev_key = writes[0]["rkey"].as_str().unwrap();
    let p = keys::revision_prefix(NS, &uri).unwrap();
    assert!(rev_key.starts_with(&format!("{p}-")));
    let rev = w.alice.get(nsid::REVISION, rev_key).unwrap();
    assert_eq!(rev["subject"], json!({"uri": uri, "cid": base}));
    assert_eq!(rev["op"], "update");
    assert_eq!(rev["record"], old);
    assert_eq!(rev["summary"], "typo");
}

#[test]
fn edit_of_a_changed_record_is_a_conflict() {
    let w = world();
    let (uri, base) = w.seed_post("one");
    let rkey = rkey_of(&uri);
    let old = w.alice.get(nsid::POST, &rkey).unwrap();
    // edited elsewhere before this edit is sent
    let mut other = old.clone();
    other["title"] = json!("edited elsewhere");
    w.alice.put(nsid::POST, &rkey, &other);
    let mut new = old.clone();
    new["title"] = json!("mine");
    let t = block_on(w.ob.edit(&w.session, &uri, &new, &base, EditOpts::default())).unwrap();
    let applies = w.net.count(APPLY);
    w.run();
    let t = w.ticket(&t);
    assert_eq!(failed_code(&t), ErrorCode::Conflict);
    let WriteState::Failed { error } = &t.state else {
        unreachable!()
    };
    assert_eq!(
        error.detail.as_ref().unwrap()["currentCid"],
        w.alice.cid(nsid::POST, &rkey).unwrap().to_string()
    );
    assert_eq!(w.net.count(APPLY), applies, "nothing sent");
    assert_eq!(
        w.alice.get(nsid::POST, &rkey).unwrap()["title"],
        "edited elsewhere"
    );
}

#[test]
fn edit_racing_another_write_ends_in_conflict_not_overwrite() {
    let w = world();
    let (uri, base) = w.seed_post("one");
    let rkey = rkey_of(&uri);
    let old = w.alice.get(nsid::POST, &rkey).unwrap();
    {
        let (net, alice, rkey, old) = (w.net.clone(), w.alice.clone(), rkey.clone(), old.clone());
        let calls = Arc::new(AtomicU32::new(0));
        w.host.on_post(&w.userpds(APPLY), move |req| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                // another device edits between our head read and our applyWrites
                let mut v = old.clone();
                v["title"] = json!("raced");
                alice.put(nsid::POST, &rkey, &v);
            }
            net.request(req, "default")
        });
    }
    let mut new = old.clone();
    new["title"] = json!("mine");
    w.host.clear_requests();
    let t = block_on(w.ob.edit(&w.session, &uri, &new, &base, EditOpts::default())).unwrap();
    w.run();
    assert_eq!(failed_code(&w.ticket(&t)), ErrorCode::Conflict);
    assert_eq!(w.alice.get(nsid::POST, &rkey).unwrap()["title"], "raced");
    assert_eq!(
        w.bodies(APPLY).len(),
        1,
        "the swap failed; the re-read found the change"
    );
}

#[test]
fn no_history_edit_is_a_compare_and_swap_put() {
    let w = world();
    let (uri, base) = w.seed_post("one");
    let rkey = rkey_of(&uri);
    let mut new = w.alice.get(nsid::POST, &rkey).unwrap();
    new["title"] = json!("quiet fix");
    let opts = EditOpts {
        history: false,
        ..EditOpts::default()
    };
    let t = block_on(w.ob.edit(&w.session, &uri, &new, &base, opts.clone())).unwrap();
    w.run();
    let (_, cid) = done(&w.ticket(&t));
    let puts = w.bodies(PUT);
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0]["swapRecord"], base);
    assert!(w.alice.list(nsid::REVISION).is_empty());
    // W5: an update sets updatedAt and keeps createdAt
    let now = w.alice.get(nsid::POST, &rkey).unwrap();
    assert!(now["updatedAt"].is_string());
    assert_eq!(now["createdAt"], new["createdAt"]);
    // a stale base is a conflict
    new["title"] = json!("stale");
    let t = block_on(w.ob.edit(&w.session, &uri, &new, &base, opts)).unwrap();
    w.run();
    assert_eq!(failed_code(&w.ticket(&t)), ErrorCode::Conflict);
    assert_eq!(w.alice.cid(nsid::POST, &rkey).unwrap().to_string(), cid);
}

// ---------------------------------------------------------------------------------------------
// Opinions: hold, coalescing, undo, two devices

fn subject(w: &World) -> (String, String) {
    let rkey = format!("{}-{}", w.fk(), tid(60_000, 0));
    let v = Post::article(w.forum(), "bob's", "hi", w.now());
    let cid = w
        .bob
        .put(nsid::POST, &rkey, &serde_json::to_value(v).unwrap());
    (w.bob.uri(nsid::POST, &rkey), cid.to_string())
}

fn karma(v: &str) -> OpinionStep {
    OpinionStep::Karma {
        value: Some(v.into()),
    }
}

fn react(id: &str, remove: bool) -> OpinionStep {
    OpinionStep::React {
        id: id.into(),
        stance: "for".into(),
        quote: None,
        remove,
    }
}

fn opinion_rkey(w: &World, subject: &str) -> String {
    records::opinion_rkey(w.forum().as_str(), subject).unwrap()
}

fn react_ids(v: &Value) -> Vec<String> {
    v["reacts"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| r["react"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn opinions_are_held_coalesced_and_written_once() {
    let w = world();
    let (s, scid) = subject(&w);
    let f = w.forum();
    let op = |step| block_on(w.ob.opinion(&w.session, f.as_str(), &s, &scid, step)).unwrap();
    let t1 = op(react("funny", false));
    let WriteState::Pending {
        held: true,
        not_before_ms: Some(until),
        ..
    } = t1.state
    else {
        panic!("{t1:?}")
    };
    assert_eq!(until, w.host.now() + 4_000);
    // nothing is sent during the hold
    assert_eq!(block_on(w.ob.run_due(&w.session)).unwrap().processed, 0);
    w.host.advance_ms(1_000);
    let t2 = op(react("wise", false));
    let t3 = op(react("funny", true));
    let t4 = op(karma("up"));
    assert_eq!(t2.id, t1.id);
    assert_eq!(t3.id, t1.id);
    assert_eq!(t4.id, t1.id);
    w.run();
    done(&w.ticket(&t1));
    assert_eq!(
        w.net.count(PUT) + w.net.count(CREATE),
        1,
        "one write for four intents"
    );
    let v = w.alice.get(nsid::OPINION, &opinion_rkey(&w, &s)).unwrap();
    assert_eq!(react_ids(&v), vec!["wise"]);
    assert_eq!(v["karma"], "up");
    assert_eq!(v["subject"]["cid"], scid);
    // the hold ran from the last intent
    assert!(w.host.now() >= until + 1_000);
}

#[test]
fn undo_within_the_window_removes_the_write() {
    let w = world();
    let (s, scid) = subject(&w);
    let f = w.forum();
    let t = block_on(w.ob.opinion(&w.session, f.as_str(), &s, &scid, karma("up"))).unwrap();
    let u = block_on(w.ob.undo(&w.session, &t.id)).unwrap();
    assert_eq!(u.id, t.id);
    assert_eq!(u.state, WriteState::Undone);
    w.run();
    assert_eq!(w.net.count(PUT) + w.net.count(CREATE), 0);
    assert!(w.alice.get(nsid::OPINION, &opinion_rkey(&w, &s)).is_none());
    // undoing again is idempotent
    assert_eq!(
        block_on(w.ob.undo(&w.session, &t.id)).unwrap().state,
        WriteState::Undone
    );
}

#[test]
fn undo_after_sending_restores_the_prior_opinion() {
    let w = world();
    let (s, scid) = subject(&w);
    let f = w.forum();
    let op = |step| block_on(w.ob.opinion(&w.session, f.as_str(), &s, &scid, step)).unwrap();
    let rkey = opinion_rkey(&w, &s);
    // prior: a react
    let t0 = op(react("funny", false));
    w.run();
    done(&w.ticket(&t0));
    // karma up, sent
    let t1 = op(karma("up"));
    w.run();
    done(&w.ticket(&t1));
    assert_eq!(w.alice.get(nsid::OPINION, &rkey).unwrap()["karma"], "up");
    // undo issues the inverse as a new write
    let u = block_on(w.ob.undo(&w.session, &t1.id)).unwrap();
    assert_ne!(u.id, t1.id);
    let inv = block_on(w.ob.get(&u.id)).unwrap().unwrap();
    assert_eq!(inv.undo_of.as_deref(), Some(t1.id.as_str()));
    w.run();
    done(&w.ticket(&u));
    let v = w.alice.get(nsid::OPINION, &rkey).unwrap();
    assert!(v.get("karma").is_none());
    assert_eq!(react_ids(&v), vec!["funny"]);
    // undoing the very first opinion empties it: the record is deleted
    let u0 = block_on(w.ob.undo(&w.session, &t0.id)).unwrap();
    w.run();
    done(&w.ticket(&u0));
    assert!(w.alice.get(nsid::OPINION, &rkey).is_none());
    assert_eq!(w.net.count(DELETE), 1);
    // a sent post cannot be undone
    let (uri, _) = w.seed_post("p");
    let posts = block_on(w.ob.writes(w.alice.did().as_str())).unwrap();
    let id = posts
        .iter()
        .find(|p| p.primary_uri().as_deref() == Some(uri.as_str()))
        .unwrap()
        .id();
    assert_eq!(
        block_on(w.ob.undo(&w.session, &id)).unwrap_err().code,
        ErrorCode::InvalidInput
    );
}

#[test]
fn opinions_from_two_devices_merge() {
    let w = world();
    let (s, scid) = subject(&w);
    let f = w.forum();
    // device B: its own host (stores, clock), same account and network
    let hb = MemHost::new();
    hb.set_now_ms(w.host.now());
    hb.set_seed(9);
    w.net.install_as(&hb, "b");
    let ob_b = Outbox::new(Arc::new(hb.clone()));
    let rkey = opinion_rkey(&w, &s);

    let ta = block_on(w.ob.opinion(&w.session, f.as_str(), &s, &scid, karma("up"))).unwrap();
    let tb =
        block_on(ob_b.opinion(&w.session, f.as_str(), &s, &scid, react("funny", false))).unwrap();
    w.run();
    block_on(ob_b.run_until_idle(&w.session)).unwrap();
    done(&w.ticket(&ta));
    done(&block_on(ob_b.ticket(&tb.id)).unwrap().unwrap());
    let v = w.alice.get(nsid::OPINION, &rkey).unwrap();
    assert_eq!(v["karma"], "up");
    assert_eq!(react_ids(&v), vec!["funny"]);

    // a race: B writes between A's read and A's put; A's swap fails, A re-reads and re-applies
    {
        let (net, alice, rkey, prior) = (w.net.clone(), w.alice.clone(), rkey.clone(), v.clone());
        let calls = Arc::new(AtomicU32::new(0));
        w.host.on_post(&w.userpds(PUT), move |req| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut b = prior.clone();
                b["reacts"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"react": "wise", "stance": "for"}));
                alice.put(nsid::OPINION, &rkey, &b);
            }
            net.request(req, "default")
        });
    }
    let sleeps = w.host.sleeps().len();
    w.host.clear_requests();
    let ta = block_on(w.ob.opinion(
        &w.session,
        f.as_str(),
        &s,
        &scid,
        react("insightful", false),
    ))
    .unwrap();
    w.run();
    done(&w.ticket(&ta));
    let v = w.alice.get(nsid::OPINION, &rkey).unwrap();
    assert_eq!(
        react_ids(&v),
        vec!["funny", "wise", "insightful"],
        "deterministic order"
    );
    assert_eq!(v["karma"], "up");
    assert_eq!(w.bodies(PUT).len(), 2);
    assert!(
        w.host.sleeps()[sleeps..]
            .iter()
            .any(|ms| (200..=800).contains(ms))
    );
}

// ---------------------------------------------------------------------------------------------
// Delete protocol

#[test]
fn delete_sends_the_target_first_in_chunks_of_200() {
    let w = world();
    let (uri, cid) = w.seed_post("doomed");
    let (other, _) = w.seed_post("kept");
    let p = keys::revision_prefix(NS, &uri).unwrap();
    let rev = |subject: &str, n: u64| {
        json!({
            "$type": nsid::REVISION,
            "subject": {"uri": subject, "cid": cid},
            "op": "update",
            "record": {"$type": nsid::POST, "n": n},
            "createdAt": "2026-01-01T00:00:00.000Z",
        })
    };
    let revs: Vec<String> = (0..450)
        .map(|i| format!("{p}-{}", tid(3_600_000, i)))
        .collect();
    let spoof = format!("{p}-{}", tid(3_600_000, 999));
    let nested = format!("{p}-{}-{}", tid(3_600_000, 1_000), tid(3_600_000, 1_001));
    w.alice.batch(|wr| {
        for (i, k) in revs.iter().enumerate() {
            wr.put(nsid::REVISION, k, &rev(&uri, i as u64));
        }
        // same prefix, another subject: kept (§3.2)
        wr.put(nsid::REVISION, &spoof, &rev(&other, 0));
        // not exactly P + "-" + TID: kept
        wr.put(nsid::REVISION, &nested, &rev(&uri, 1));
    });
    w.host.clear_requests();
    let t = block_on(w.ob.delete(&w.session, &uri)).unwrap();
    w.run();
    done(&w.ticket(&t));
    let bodies = w.bodies(APPLY);
    let sizes: Vec<usize> = bodies
        .iter()
        .map(|b| b["writes"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, vec![200, 200, 51]);
    let first = &bodies[0]["writes"][0];
    assert_eq!(first["$type"], "com.atproto.repo.applyWrites#delete");
    assert_eq!(first["collection"], nsid::POST);
    assert_eq!(first["rkey"], rkey_of(&uri));
    assert!(bodies.iter().all(|b| b.get("swapCommit").is_none()));
    assert!(w.alice.get(nsid::POST, &rkey_of(&uri)).is_none());
    let left: Vec<String> = w
        .alice
        .list(nsid::REVISION)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let mut want = vec![spoof, nested];
    want.sort();
    let mut left_sorted = left.clone();
    left_sorted.sort();
    assert_eq!(left_sorted, want);
    assert!(!w.cached(&uri), "tombstoned in the store");
}

#[test]
fn a_partial_delete_keeps_retrying_the_orphans() {
    let w = world();
    let (uri, cid) = w.seed_post("doomed");
    let p = keys::revision_prefix(NS, &uri).unwrap();
    w.alice.batch(|wr| {
        for i in 0..250 {
            wr.put(
                nsid::REVISION,
                &format!("{p}-{}", tid(3_600_000, i)),
                &json!({
                    "$type": nsid::REVISION,
                    "subject": {"uri": uri, "cid": cid},
                    "op": "update",
                    "record": {"n": i},
                    "createdAt": "2026-01-01T00:00:00.000Z",
                }),
            );
        }
    });
    // the second chunk fails ambiguously
    w.net
        .inject(Fault::status(502).on_nsid(APPLY).after(1).once());
    let t = block_on(w.ob.delete(&w.session, &uri)).unwrap();
    w.run();
    done(&w.ticket(&t));
    assert!(w.alice.get(nsid::POST, &rkey_of(&uri)).is_none());
    assert!(w.alice.list(nsid::REVISION).is_empty());
}

// ---------------------------------------------------------------------------------------------
// Budget and rate limits

#[test]
fn a_429_throttles_until_retry_after() {
    let w = world();
    w.net.inject(Fault::rate_limited(30).on_nsid(APPLY).once());
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    let start = w.host.now();
    let r = block_on(w.ob.run_due(&w.session)).unwrap();
    assert_eq!(r.throttled, 1);
    let WriteState::Pending {
        throttled: true,
        not_before_ms: Some(until),
        ..
    } = w.ticket(&t).state
    else {
        panic!()
    };
    assert!(until >= start + 30_000, "{until} vs {start}");
    let budget = block_on(w.ob.budget(w.alice.did().as_str())).unwrap();
    assert!(budget.blocked_until >= start + 30_000);
    // nothing is sent before then
    assert_eq!(block_on(w.ob.step(&w.session)).unwrap(), Step::Wait(until));
    w.run();
    done(&w.ticket(&t));
    assert!(w.host.now() >= until);
    assert_eq!(w.net.count(APPLY), 2);
}

#[test]
fn the_write_budget_throttles_before_sending() {
    let w = world();
    let did = w.alice.did().as_str().to_owned();
    let now = w.host.now();
    let bucket = now - now.rem_euclid(lessdugin_core::outbox::budget::BUCKET_MS);
    let row = BudgetRow {
        spent: vec![(bucket, 4_998)],
        ..BudgetRow::default()
    };
    block_on(w.host.state_kv().write(vec![KvOp::Put(
        model::budget_key(&did),
        row::encode(&row).unwrap(),
    )]))
    .unwrap();
    let t = block_on(w.ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    assert!(matches!(
        block_on(w.ob.step(&w.session)).unwrap(),
        Step::Wait(_)
    ));
    let WriteState::Pending {
        throttled: true,
        not_before_ms: Some(until),
        ..
    } = w.ticket(&t).state
    else {
        panic!()
    };
    let hour = lessdugin_core::outbox::budget::HOUR_MS;
    assert_eq!(
        until,
        bucket + lessdugin_core::outbox::budget::BUCKET_MS + hour
    );
    assert_eq!(w.net.count(APPLY), 0);
    w.run();
    done(&w.ticket(&t));
    let b = block_on(w.ob.budget(&did)).unwrap();
    assert_eq!(b.window_points(w.host.now(), hour), 6, "two creates");
}

// ---------------------------------------------------------------------------------------------
// Create-if-absent, blobs, moderation, W9

#[test]
fn join_falls_back_to_create_record_on_hosts_without_null_swap() {
    let w = world();
    w.net.inject(
        Fault::error(400, "InvalidRequest", "swapRecord must be a cid")
            .on_nsid(PUT)
            .once(),
    );
    let t = block_on(w.ob.join(&w.session, &w.forum(), Some("hello"))).unwrap();
    w.run();
    done(&w.ticket(&t));
    assert_eq!((w.net.count(PUT), w.net.count(CREATE)), (1, 1));
    assert_eq!(
        w.alice.get(nsid::MEMBERSHIP, &w.fk()).unwrap()["note"],
        "hello"
    );
    // remembered for the host; an existing record (createRecord 500) is success
    let t = block_on(w.ob.join(&w.session, &w.forum(), None)).unwrap();
    w.run();
    done(&w.ticket(&t));
    assert_eq!((w.net.count(PUT), w.net.count(CREATE)), (1, 2));
    // leave: deleteRecord with the stored CID as swapRecord
    let cid = w.alice.cid(nsid::MEMBERSHIP, &w.fk()).unwrap().to_string();
    let t = block_on(w.ob.leave(&w.session, &w.forum())).unwrap();
    w.run();
    done(&w.ticket(&t));
    assert_eq!(w.bodies(DELETE)[0]["swapRecord"], cid);
    assert!(w.alice.get(nsid::MEMBERSHIP, &w.fk()).is_none());
}

#[test]
fn long_bodies_spill_to_markdown_blobs_uploaded_at_send() {
    let w = world();
    let body = "word ".repeat(14_000); // 70,000 bytes
    let post = Post::article(w.forum(), "long", body.clone(), w.now());
    let t = block_on(w.ob.create_post(
        &w.session,
        &post,
        ContentOpts {
            excerpt: Some("A long post.".into()),
            ..ContentOpts::default()
        },
    ))
    .unwrap();
    let stored = block_on(w.ob.get(&t.id)).unwrap().unwrap();
    assert_eq!(stored.blobs.len(), 1);
    assert_eq!(stored.blobs[0].mime, "text/markdown");
    assert_eq!(
        w.net.count("com.atproto.repo.uploadBlob"),
        0,
        "uploaded only when sent"
    );
    w.run();
    let (uri, _) = done(&w.ticket(&t));
    assert_eq!(w.net.count("com.atproto.repo.uploadBlob"), 1);
    let v = w.alice.get(nsid::POST, &rkey_of(&uri)).unwrap();
    assert!(v["body"].get("text").is_none());
    assert_eq!(v["body"]["textBlob"]["ref"]["$link"], stored.blobs[0].cid);
    assert_eq!(v["body"]["textBlob"]["mimeType"], "text/markdown");
    assert_eq!(v["excerpt"], "A long post.");
    // the blob chunks are gone once done
    let rows = block_on(w.host.state_kv().scan(
        lessdugin_core::host::KeyRange::prefix(&lessdugin_core::key!(model::OUT_BLOB)),
        lessdugin_core::host::Dir::Forward,
        10,
    ))
    .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn blobs_are_uploaded_again_after_a_long_wait() {
    use lessdugin_core::outbox::BlobData;
    use lessdugin_lexicon::records::Profile;

    let w = world();
    let png = BlobData::media(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR avatar".to_vec()).unwrap();
    let profile = Profile {
        display_name: Some("Alice".into()),
        avatar: Some(serde_json::from_value(png.placeholder()).unwrap()),
        ..Profile::default()
    };
    // the record write is rate limited for an hour, after the blob went up
    w.net.inject(Fault::rate_limited(3_600).on_nsid(PUT).once());
    let t = block_on(w.ob.put_profile(&w.session, &profile, None, vec![png.clone()])).unwrap();
    assert_eq!(block_on(w.ob.run_due(&w.session)).unwrap().throttled, 1);
    assert_eq!(w.net.count("com.atproto.repo.uploadBlob"), 1);
    // meanwhile the PDS collects the unreferenced blob
    assert_eq!(w.net.gc_blobs(), 1);
    w.run();
    done(&w.ticket(&t));
    assert_eq!(
        w.net.count("com.atproto.repo.uploadBlob"),
        2,
        "uploaded again"
    );
    let v = w.alice.get(nsid::PROFILE, "self").unwrap();
    assert_eq!(v["avatar"]["ref"]["$link"], png.cid);
    assert_eq!(v["displayName"], "Alice");
}

#[test]
fn mod_assertions_continue_as_updates_and_removals_snapshot() {
    let w = world();
    let (target, tcid) = subject(&w);
    let mk = |reason: &str| {
        let mut m = ModAction::new(
            w.forum(),
            target.as_str(),
            Action::Hide(HideAction::hide()),
            w.now(),
        );
        m.subject_cid = Some(tcid.clone());
        m.reason = Some(reason.into());
        m
    };
    // an older assertion with the same key exists
    let old = records::mod_action(&mk("old")).unwrap();
    w.alice.put(nsid::MOD_ACTION, &old.rkey, &old.value);
    let old_cid = w
        .alice
        .cid(nsid::MOD_ACTION, &old.rkey)
        .unwrap()
        .to_string();
    w.host.advance_ms(60_000);
    let t = block_on(w.ob.mod_assert(&w.session, &mk("new"))).unwrap();
    w.run();
    let (uri, _) = done(&w.ticket(&t));
    assert_eq!(rkey_of(&uri), old.rkey);
    let v = w.alice.get(nsid::MOD_ACTION, &old.rkey).unwrap();
    assert_eq!(v["reason"], "new");
    assert_eq!(v["createdAt"], old.value["createdAt"], "keeps createdAt");
    assert!(v["updatedAt"].is_string());
    let revs = w.alice.list(nsid::REVISION);
    assert_eq!(revs.len(), 1);
    let r = w.alice.get(nsid::REVISION, &revs[0].0).unwrap();
    assert_eq!(r["subject"]["cid"], old_cid);
    assert_eq!(r["op"], "update");
    // removal: a delete revision and the delete, in one swapped batch
    let t = block_on(w.ob.delete(&w.session, &uri)).unwrap();
    w.run();
    done(&w.ticket(&t));
    assert!(w.alice.get(nsid::MOD_ACTION, &old.rkey).is_none());
    let ops: Vec<String> = w
        .alice
        .list(nsid::REVISION)
        .iter()
        .map(|(k, _)| {
            w.alice.get(nsid::REVISION, k).unwrap()["op"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(ops.len(), 2);
    assert!(ops.contains(&"delete".to_owned()));
    // a lock with more than 5,000 allowed comments is refused
    let allowed: Vec<AtUri> = (0..5_001u64)
        .map(|i| {
            AtUri::parse(&format!(
                "at://{}/{}/{}-{}-{}",
                w.bob.did(),
                nsid::COMMENT,
                w.fk(),
                tid(1, 0),
                tid(1, i)
            ))
            .unwrap()
        })
        .collect();
    let lock = ModAction::new(
        w.forum(),
        target.as_str(),
        Action::Lock(lessdugin_lexicon::records::LockAction::new(allowed)),
        w.now(),
    );
    let e = block_on(w.ob.mod_assert(&w.session, &lock)).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
}

#[test]
fn mod_adopt_reissues_the_former_moderators_assertions() {
    use lessdugin_lexicon::records::TierAction;
    use lessdugin_lexicon::values::Tier;

    let w = world();
    let carol = w.net.add_account("carol.example.com");
    let (target, tcid) = subject(&w);
    let assertion = |action: Action, reason: &str| {
        let mut m = ModAction::new(w.forum(), target.as_str(), action, w.now());
        m.subject_cid = Some(tcid.clone());
        m.reason = Some(reason.into());
        records::mod_action(&m).unwrap()
    };
    let hide = assertion(Action::Hide(HideAction::hide()), "spam");
    let curated = assertion(Action::Tier(TierAction::new(Tier::Curated)), "great");
    carol.put(nsid::MOD_ACTION, &hide.rkey, &hide.value);
    carol.put(nsid::MOD_ACTION, &curated.rkey, &curated.value);
    let adopt = || {
        let t = block_on(w.ob.mod_adopt(
            &w.session,
            &w.forum(),
            carol.did(),
            &carol.pds_base(),
            "moderator",
        ))
        .unwrap();
        w.run();
        done(&w.ticket(&t));
        block_on(w.ob.get(&t.id)).unwrap().unwrap()
    };
    w.host.advance_ms(60_000);
    let first = adopt();
    let mine = w.alice.get(nsid::MOD_ACTION, &hide.rkey).unwrap();
    assert_eq!(
        mine["adopts"],
        json!({
            "uri": carol.uri(nsid::MOD_ACTION, &hide.rkey),
            "cid": carol.cid(nsid::MOD_ACTION, &hide.rkey).unwrap().to_string(),
        })
    );
    assert_eq!(mine["reason"], "spam");
    assert_ne!(
        mine["createdAt"], hide.value["createdAt"],
        "createdAt = now"
    );
    assert!(mine.get("updatedAt").is_none());
    // a moderator may not issue a curated tier
    assert!(w.alice.get(nsid::MOD_ACTION, &curated.rkey).is_none());
    let Intent::Adopt { report, .. } = first.intent else {
        panic!("{:?}", first.intent)
    };
    assert_eq!(report.len(), 1);
    assert!(report[0].starts_with("cannot adopt"), "{report:?}");

    // the original changes: the adopted copy is updated with a revision snapshot
    let mut changed = hide.value.clone();
    changed["reason"] = json!("spam, really");
    carol.put(nsid::MOD_ACTION, &hide.rkey, &changed);
    w.host.advance_ms(60_000);
    adopt();
    let mine2 = w.alice.get(nsid::MOD_ACTION, &hide.rkey).unwrap();
    assert_eq!(mine2["reason"], "spam, really");
    assert_eq!(mine2["createdAt"], mine["createdAt"]);
    assert!(mine2["updatedAt"].is_string());
    let revs = w.alice.list(nsid::REVISION);
    assert_eq!(revs.len(), 1);
    let r = w.alice.get(nsid::REVISION, &revs[0].0).unwrap();
    assert_eq!(r["op"], "update");
    assert_eq!(r["record"], mine);

    // nothing new: nothing is sent
    let applies = w.net.count(APPLY);
    adopt();
    assert_eq!(w.net.count(APPLY), applies);
}

#[test]
fn writes_are_never_sent_for_another_account() {
    let w = world();
    let bobs = w.bob.uri(nsid::POST, &format!("{}-{}", w.fk(), tid(1, 0)));
    let e = block_on(w.ob.delete(&w.session, &bobs)).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    let e = block_on(w.ob.undo(&w.session, &format!("{}/1", w.bob.did()))).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    // a row under this account stamped with another one is failed, not sent
    let did = w.alice.did().as_str();
    let stamped = PendingWrite {
        v: OUTBOX_ROW_VERSION,
        account: w.bob.did().as_str().to_owned(),
        seq: 1,
        intent: Intent::DeleteRecord,
        ops: vec![Op::delete(nsid::MEMBERSHIP, &w.fk())],
        blobs: Vec::new(),
        state: WriteStage::Queued,
        attempts: 0,
        created_at: 0,
        updated_at: 0,
        not_before: None,
        sent: false,
        results: Vec::new(),
        error: None,
        undo_of: None,
    };
    block_on(w.host.state_kv().write(vec![KvOp::Put(
        model::write_key(did, 1),
        row::encode(&stamped).unwrap(),
    )]))
    .unwrap();
    let r = block_on(w.ob.run_until_idle(&w.session)).unwrap();
    assert_eq!(r.failed, 1);
    assert_eq!(w.net.count(DELETE), 0);
    let row = block_on(w.ob.get(&format!("{did}/1"))).unwrap().unwrap();
    assert_eq!(row.state, WriteStage::Failed);
    assert!(row.error.unwrap().message.contains("W9"));
    // a generic write with an invalid value is refused before it is persisted
    let bad = NewWrite::new(
        Intent::Create,
        vec![Op::create(
            nsid::POST,
            "not-a-key",
            &json!({"$type": nsid::POST}),
        )],
    );
    assert_eq!(
        block_on(w.ob.enqueue(&w.session, bad)).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert_eq!(block_on(w.ob.writes(did)).unwrap().len(), 1);
}

#[test]
fn retry_requeues_a_failed_write_and_events_are_reported() {
    use std::sync::Mutex;

    #[derive(Default)]
    struct Rec(Mutex<Vec<WriteTicket>>, Mutex<Vec<String>>);
    impl lessdugin_core::outbox::OutboxListener for Rec {
        fn on_write(&self, t: &WriteTicket) {
            self.0.lock().unwrap().push(t.clone());
        }
        fn on_applied(
            &self,
            a: &lessdugin_core::outbox::AppliedWrite,
            _: &lessdugin_core::outbox::ApplyReport,
        ) {
            self.1.lock().unwrap().push(a.rev.clone());
        }
    }
    let w = world();
    let rec = Arc::new(Rec::default());
    let ob = Outbox::with_listener(Arc::new(w.host.clone()), rec.clone());
    w.net.inject(
        Fault::error(400, "InvalidRequest", "nope")
            .on_nsid(APPLY)
            .once(),
    );
    let t = block_on(ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    block_on(ob.run_until_idle(&w.session)).unwrap();
    let failed = block_on(ob.ticket(&t.id)).unwrap().unwrap();
    assert_eq!(failed_code(&failed), ErrorCode::InvalidInput);
    let again = block_on(ob.retry(&w.session, &t.id)).unwrap();
    assert!(matches!(again.state, WriteState::Pending { .. }));
    block_on(ob.run_until_idle(&w.session)).unwrap();
    done(&block_on(ob.ticket(&t.id)).unwrap().unwrap());
    let kinds: Vec<&'static str> = rec
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|t| match t.state {
            WriteState::Pending { .. } => "pending",
            WriteState::Sending => "sending",
            WriteState::Done { .. } => "done",
            WriteState::Failed { .. } => "failed",
            WriteState::Undone => "undone",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["pending", "sending", "failed", "pending", "sending", "done"]
    );
    let revs = rec.1.lock().unwrap().clone();
    assert_eq!(revs.len(), 1);
    assert!(!revs[0].is_empty());
    // done writes can be pruned
    let n = block_on(ob.prune(w.alice.did().as_str(), w.host.now() + 1)).unwrap();
    assert_eq!(n, 1);
    assert!(block_on(ob.get(&t.id)).unwrap().is_none());
}

#[test]
fn the_long_lived_pump_drains_and_stops() {
    use std::sync::OnceLock;

    struct StopWhenDone(OnceLock<Outbox<MemHost>>);
    impl lessdugin_core::outbox::OutboxListener for StopWhenDone {
        fn on_write(&self, t: &WriteTicket) {
            if matches!(t.state, WriteState::Done { .. })
                && let Some(ob) = self.0.get()
            {
                ob.stop();
            }
        }
    }
    let w = world();
    let l = Arc::new(StopWhenDone(OnceLock::new()));
    let ob = Outbox::with_listener(Arc::new(w.host.clone()), l.clone());
    assert!(l.0.set(ob.clone()).is_ok());
    let t = block_on(ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    block_on(ob.pump(&w.session));
    done(&block_on(ob.ticket(&t.id)).unwrap().unwrap());
}

#[test]
fn a_lost_response_is_recognised_by_its_cid_and_cached() {
    let w = world();
    // the membership is known locally: a plain createRecord
    let (first, _) = w.seed_post("one");
    assert!(w.cached(&first));
    {
        let net = w.net.clone();
        let calls = Arc::new(AtomicU32::new(0));
        w.host.on_post(&w.userpds(CREATE), move |req| {
            let r = net.request(req, "default");
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                // applied, but the answer is lost
                return Ok(lessdugin_testkit::resp::xrpc_error(
                    502,
                    "UpstreamFailure",
                    "Bad Gateway",
                ));
            }
            r
        });
    }
    let t = block_on(w.ob.create_post(&w.session, &w.post("two"), ContentOpts::default())).unwrap();
    w.run();
    let (uri, cid) = done(&w.ticket(&t));
    // (the seed was first content: an applyWrites)
    assert_eq!(w.net.count(CREATE), 1, "sent once, never resent");
    assert_eq!(
        w.alice.cid(nsid::POST, &rkey_of(&uri)).unwrap().to_string(),
        cid
    );
    assert!(w.cached(&uri), "the re-read value is applied to the store");
}

#[test]
fn with_a_syncer_own_writes_queue_the_own_repo() {
    use lessdugin_core::engine::EngineConfig;
    use lessdugin_core::sync::{Priority, Reason, SyncConfig, Syncer};

    let w = world();
    let sy = Syncer::new(
        Arc::new(w.host.clone()),
        SyncConfig::from_engine(&EngineConfig::default()),
    );
    let ob = Outbox::new(Arc::new(w.host.clone())).with_syncer(sy.clone());
    let t = block_on(ob.create_post(&w.session, &w.post("one"), ContentOpts::default())).unwrap();
    block_on(ob.run_until_idle(&w.session)).unwrap();
    let (uri, _) = done(&block_on(ob.ticket(&t.id)).unwrap().unwrap());
    assert!(w.cached(&uri));
    let membership = format!("at://{}/{}/{}", w.alice.did(), nsid::MEMBERSHIP, w.fk());
    assert!(w.cached(&membership));
    let q = sy.queue_snapshot();
    let item = q.get(w.alice.did().as_str()).expect("own repo queued");
    assert!(item.reasons.contains(&Reason::OwnWrite));
    assert_eq!(item.priority, Priority::P1);
    assert!(item.min_rev.is_some());
    let repo = block_on(RecordStore::new(w.host.cache_kv()).repo(w.alice.did().as_str()))
        .unwrap()
        .unwrap();
    assert!(repo.head_seen.is_some(), "headSeen follows the commit");
}

/// The outbox's futures are `Send` on native targets (the engine runs them on any executor).
#[allow(dead_code)]
fn futures_are_send(ob: &Outbox<MemHost>, s: &Session, p: &Post) {
    fn send<T: Send>(_: T) {}
    send(ob.pump(s));
    send(ob.step(s));
    send(ob.run_until_idle(s));
    send(ob.create_post(s, p, ContentOpts::default()));
    send(ob.opinion(s, "", "", "", karma("up")));
    send(ob.undo(s, ""));
    send(ob.edit(s, "", &Value::Null, "", EditOpts::default()));
}
