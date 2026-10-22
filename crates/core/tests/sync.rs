// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-repo sync (`sync.md` §5) and discovery (§4) against the fake network (`FakeNet` on a
//! `MemHost`): range listing and its fallbacks, rollback, tombstones, caps, rate limits,
//! breakers, the approval/open rosters, fail-closed degradation and the cold-sync cost model.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_core::discovery::{DeclProof, DiscState, ForumKey, ForumSync, SyncOpts};
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_core::host::Host;
use lessdugin_core::store::repo::BreakerState;
use lessdugin_core::store::repo::RangeCursor;
use lessdugin_core::store::{RecKey, RecordStore, RepoRow, Slot};
use lessdugin_core::sync::{
    Priority, Reason, RepoOutcome, RepoReport, Session, Stats, SyncConfig, SyncRequest, Syncer,
};
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::records::{
    AccessAction, Action, Forum, Markdown, Md, Membership, ModAction, Moderator, Opinion, Post,
    StrongRef,
};
use lessdugin_lexicon::values::{AccessLevel, MembershipMode};
use lessdugin_testkit::fake_pds::{AccountHandle, CursorMode, Fault, PdsHandle, ServiceKind};
use lessdugin_testkit::repo::tid_at_ms;
use lessdugin_testkit::{FakeNet, MemHost, resp};
use serde_json::{Value, json};

const LIST: &str = "com.atproto.repo.listRecords";
const HEAD: &str = "com.atproto.sync.getLatestCommit";

fn block_on<F: Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

struct World {
    host: MemHost,
    net: FakeNet,
    engine: Engine<MemHost>,
    seq: Cell<i64>,
}

impl World {
    fn new() -> Self {
        let host = MemHost::new();
        let net = FakeNet::on(&host);
        // accounts made with `add_account` live on the first PDS: make it the default one even
        // when a test adds another PDS before its first account
        let _ = net.default_pds();
        let engine = Engine::new(host.clone(), EngineConfig::default());
        Self {
            host,
            net,
            engine,
            seq: Cell::new(0),
        }
    }

    fn syncer(&self) -> Syncer<MemHost> {
        self.engine.syncer()
    }

    /// A syncer with its own scheduler and listing parameters.
    fn syncer_with(&self, f: impl FnOnce(&mut SyncConfig)) -> Syncer<MemHost> {
        let mut c = SyncConfig::from_engine(self.engine.config());
        f(&mut c);
        Syncer::new(Arc::new(self.host.clone()), c)
    }

    fn dt(&self) -> Datetime {
        Datetime::from_millis(self.host.now() - 3_600_000).unwrap()
    }

    fn tid(&self) -> String {
        self.seq.set(self.seq.get() + 1);
        tid_at_ms(self.host.now() - 3_600_000 + self.seq.get()).to_string()
    }

    fn put(&self, a: &AccountHandle, coll: &str, rkey: &str, v: &Value) -> String {
        let uri = AtUri::parse(&a.uri(coll, rkey)).unwrap();
        let validity = lessdugin_lexicon::validity::check_uri(&uri, v);
        assert!(validity.first_rule().is_none(), "{uri}: {validity:?}");
        a.put(coll, rkey, v);
        uri.to_string()
    }

    fn forum(
        &self,
        owner: &AccountHandle,
        slug: &str,
        mode: MembershipMode,
        mods: &[(&AccountHandle, &str)],
    ) -> ForumKey {
        let key = ForumKey::new(owner.did(), slug).unwrap();
        let mut f = Forum::create(slug, self.dt(), mode);
        if !mods.is_empty() {
            f.moderators = Some(
                mods.iter()
                    .map(|(a, role)| Moderator {
                        did: a.did().clone(),
                        role: (*role).into(),
                        title: None,
                        extra: Default::default(),
                    })
                    .collect(),
            );
        }
        self.put(owner, nsid::FORUM, slug, &serde_json::to_value(&f).unwrap());
        self.join(owner, &key);
        key
    }

    fn join(&self, a: &AccountHandle, f: &ForumKey) {
        let m = Membership::new(f.uri.clone(), self.dt());
        self.put(
            a,
            nsid::MEMBERSHIP,
            &f.fk,
            &serde_json::to_value(&m).unwrap(),
        );
    }

    fn access(&self, actor: &AccountHandle, f: &ForumKey, subject: &Did, level: AccessLevel) {
        let action = Action::Access(AccessAction::new(level));
        let m = ModAction::new(f.uri.clone(), subject.as_str(), action, self.dt());
        let v = serde_json::to_value(&m).unwrap();
        let am = v["action"].as_object().unwrap().clone();
        let rkey = keys::mod_action_rkey(NS, f.uri.as_str(), None, subject.as_str(), &am).unwrap();
        self.put(actor, nsid::MOD_ACTION, &rkey, &v);
    }

    /// A post; returns its uri and CID.
    fn post(&self, a: &AccountHandle, f: &ForumKey) -> (String, String) {
        let p = Post::article(f.uri.clone(), "A title", "Some body text.", self.dt());
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

    fn sync_forum(&self, f: &ForumKey, opts: &SyncOpts) -> ForumSync {
        block_on(self.syncer().sync_forum(f.uri.as_str(), opts)).unwrap()
    }

    fn session(&self, sy: &Syncer<MemHost>) -> Session<MemHost> {
        sy.session(Priority::P1, Arc::new(Stats::new()))
    }

    fn sync_repo(&self, sy: &Syncer<MemHost>, a: &AccountHandle, f: &ForumKey) -> RepoReport {
        self.sync_repo_for(sy, a, f, Reason::Participant)
    }

    fn sync_repo_for(
        &self,
        sy: &Syncer<MemHost>,
        a: &AccountHandle,
        f: &ForumKey,
        reason: Reason,
    ) -> RepoReport {
        let s = self.session(sy);
        block_on(sy.sync_repo(&s, &SyncRequest::forum(a.did().clone(), &f.fk, reason)))
    }

    fn store(&self) -> RecordStore<'_, lessdugin_core::store::mem::MemKv> {
        RecordStore::new(self.host.cache())
    }

    fn repo(&self, a: &AccountHandle) -> RepoRow {
        block_on(self.store().repo(a.did().as_str()))
            .unwrap()
            .unwrap()
    }

    fn slot(&self, uri: &str) -> Slot {
        let k = RecKey::of_uri(&AtUri::parse(uri).unwrap());
        block_on(self.store().slot(&k)).unwrap()
    }

    fn stored(&self, f: &ForumKey, a: &AccountHandle) -> BTreeSet<String> {
        block_on(self.store().scan_forum(&f.fk, None).collect())
            .unwrap()
            .into_iter()
            .filter(|r| r.key.did == a.did().as_str())
            .map(|r| r.uri())
            .collect()
    }

    fn requests_about(&self, did: &Did) -> usize {
        self.net
            .requests()
            .iter()
            .filter(|r| r.did.as_deref() == Some(did.as_str()))
            .count()
    }
}

fn host_row(w: &World, pds: &str) -> lessdugin_core::store::HostRow {
    block_on(w.store().host_caps(pds)).unwrap().unwrap()
}

fn is_rec(s: &Slot) -> bool {
    matches!(s, Slot::Rec(_))
}

// ---------------------------------------------------------------------------------------------
// sync_repo

#[test]
fn sync_is_idempotent() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let p = w.post(&owner, &f);
    w.opinion(&owner, &f, &p);
    w.comment(&owner, &f, &p);
    let sy = w.syncer();
    let r1 = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r1.outcome, RepoOutcome::Applied, "{r1:?}");
    assert_eq!(r1.changed, 4, "post, opinion, comment, membership");
    let rows1 = block_on(w.store().scan_forum(&f.fk, None).collect()).unwrap();
    let r2 = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r2.outcome, RepoOutcome::Clean);
    assert_eq!(r2.changed, 0);
    // a forced re-list changes nothing either
    let r3 = w.sync_repo_for(&sy, &owner, &f, Reason::Reconcile);
    assert_eq!(r3.outcome, RepoOutcome::Clean);
    w.net.clear_requests();
    let r4 = w.sync_repo_for(&sy, &owner, &f, Reason::Full);
    assert_eq!(r4.outcome, RepoOutcome::Applied);
    assert_eq!(r4.changed, 0);
    let rows4 = block_on(w.store().scan_forum(&f.fk, None).collect()).unwrap();
    assert_eq!(rows1.len(), rows4.len());
    for (a, b) in rows1.iter().zip(&rows4) {
        assert_eq!(a.key, b.key);
        assert_eq!(a.row.cid, b.row.cid);
        assert_eq!(a.row.first_seen, b.row.first_seen);
    }
    let repo = w.repo(&owner);
    assert_eq!(repo.listed_rev(&f.fk), repo.rev);
    assert_eq!(repo.rev, owner.head().1.to_string());
}

#[test]
fn cursor_range_listing_is_bounded_and_probed() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    // neighbouring forums' posts on both sides of the range
    let fa = ForumKey::new(owner.did(), "aaa").unwrap();
    let fb = ForumKey::new(owner.did(), "bbb").unwrap();
    let mine: Vec<_> = (0..5).map(|_| w.post(&owner, &f)).collect();
    let _ = (w.post(&owner, &fa), w.post(&owner, &fb));
    let sy = w.syncer_with(|c| c.tuning.limit = 2);
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    let stored = w.stored(&f, &owner);
    for (u, _) in &mine {
        assert!(stored.contains(u), "{u}");
    }
    assert_eq!(stored.len(), 6, "five posts and the membership: {stored:?}");
    let lists = w.net.requests_for(LIST);
    assert_eq!(
        lists[0].params.get("cursor").map(String::as_str),
        Some(format!("{}.", f.fk).as_str()),
        "the probe starts at {{fk}}."
    );
    // 5 keys at limit 2: three full pages, the third short (one key) — or ending at a foreign
    // key — and never a full listing
    assert!(lists.len() <= 3, "{}", lists.len());
    assert!(lists.iter().all(|l| l.params.contains_key("cursor")));
    assert_eq!(
        host_row(&w, &owner.pds_base()).range_cursor,
        RangeCursor::Yes
    );
}

fn fallback_case(mode: CursorMode) {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    // a neighbouring forum whose keys sort above `{fk}.` (so ignoring the bound shows)
    let other = (0..50)
        .map(|i| ForumKey::new(owner.did(), &format!("x{i}")).unwrap())
        .find(|k| k.fk > f.fk)
        .unwrap();
    let above = w.post(&owner, &other);
    let mine: Vec<_> = (0..3).map(|_| w.post(&owner, &f)).collect();
    owner.pds().set_cursor_mode(mode);
    let sy = w.syncer_with(|c| c.tuning.limit = 2);
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{mode:?}: {r:?}");
    let stored = w.stored(&f, &owner);
    for (u, _) in &mine {
        assert!(stored.contains(u), "{mode:?}: {u}");
    }
    assert!(!stored.contains(&above.0));
    assert_eq!(stored.len(), 4, "{mode:?}: {stored:?}");
    let h = host_row(&w, &owner.pds_base());
    assert_eq!(h.range_cursor, RangeCursor::No, "{mode:?}");
    // the second sync goes straight to FULL mode: no `{fk}.` probe
    w.net.clear_requests();
    w.post(&owner, &f);
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied);
    assert!(
        w.net
            .requests_for(LIST)
            .iter()
            .all(|l| l.params.get("cursor") != Some(&format!("{}.", f.fk))),
        "{mode:?}"
    );
    assert_eq!(w.stored(&f, &owner).len(), 5);
}

#[test]
fn a_rejected_cursor_falls_back_to_full_listing() {
    fallback_case(CursorMode::OpaqueStrict);
}

#[test]
fn an_ignored_bound_falls_back_to_full_listing() {
    fallback_case(CursorMode::IgnoresBound);
    fallback_case(CursorMode::Opaque);
}

#[test]
fn a_stuck_cursor_fails_the_listing_without_tombstones() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let posts: Vec<_> = (0..4).map(|_| w.post(&owner, &f)).collect();
    let sy = w.syncer_with(|c| c.tuning.limit = 2);
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    owner.delete(nsid::POST, posts[0].0.rsplit('/').next().unwrap());
    owner.pds().set_cursor_mode(CursorMode::Stuck);
    let r = w.sync_repo(&sy, &owner, &f);
    assert!(!r.issues.is_empty(), "{r:?}");
    // the deleted post stays until a complete listing
    assert!(is_rec(&w.slot(&posts[0].0)));
    assert!(r.strikes > 0);
}

#[test]
fn a_rollback_keeps_the_cache_until_a_full_sync() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let p = w.post(&owner, &f);
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    let rev = w.repo(&owner).rev;
    owner.delete(nsid::POST, p.0.rsplit('/').next().unwrap());
    owner.commit_at(tid_at_ms(w.host.now() - 86_400_000));
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Rollback);
    let repo = w.repo(&owner);
    assert_eq!(repo.rev, rev, "nothing applied");
    assert!(repo.rollback_since.is_some());
    assert!(repo.next_check > w.host.now(), "repo backoff");
    assert!(is_rec(&w.slot(&p.0)), "the cache is kept");
    // `sync --full` accepts the PDS state
    let r = w.sync_repo_for(&sy, &owner, &f, Reason::Full);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    let repo = w.repo(&owner);
    assert!(repo.rev < rev);
    assert!(repo.rollback_since.is_none());
    assert!(!is_rec(&w.slot(&p.0)), "the cached state was discarded");
}

#[test]
fn deletes_apply_only_after_a_complete_stable_listing() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let keep = w.post(&owner, &f);
    let gone = w.post(&owner, &f);
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    owner.delete(nsid::POST, gone.0.rsplit('/').next().unwrap());
    let (cid, rev) = owner.head();

    // the head moves during the listing: rows admitted, no tombstone, re-queued
    let moved = tid_at_ms(w.host.now() + 1_000).to_string();
    w.net.inject(
        Fault::respond(resp::ok_json(
            &json!({"cid": cid.to_string(), "rev": moved}),
        ))
        .on_repo(owner.did().as_str())
        .on_nsid(HEAD)
        .after(1)
        .once(),
    );
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Unstable, "{r:?}");
    assert!(is_rec(&w.slot(&gone.0)));
    let q = sy.queue_snapshot();
    let item = q.get(owner.did().as_str()).expect("re-queued");
    assert_eq!(item.priority, Priority::P1);
    assert_eq!(item.min_rev.as_deref(), Some(moved.as_str()));
    assert_ne!(w.repo(&owner).rev, rev.to_string(), "not marked synced");

    // a listing that fails: still no tombstone
    let fault = w.net.inject(
        Fault::status(500)
            .on_repo(owner.did().as_str())
            .on_nsid(LIST),
    );
    let r = w.sync_repo(&sy, &owner, &f);
    assert!(!r.issues.is_empty());
    assert!(is_rec(&w.slot(&gone.0)));
    assert!(w.repo(&owner).listed_rev(&f.fk) < rev.to_string().as_str());
    w.net.remove_fault(fault);

    // a complete stable listing tombstones it
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(matches!(w.slot(&gone.0), Slot::Tomb(_)));
    assert!(is_rec(&w.slot(&keep.0)));
    assert_eq!(w.repo(&owner).rev, rev.to_string());
}

#[test]
fn fetch_caps_are_complete_and_byte_caps_partial() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let mut posts: Vec<_> = (0..5).map(|_| w.post(&owner, &f)).collect();
    posts.sort();
    let sy = w.syncer_with(|c| c.tuning.fetch_cap = Some(3));
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    let stored = w.stored(&f, &owner);
    // the newest three keys (descending order) are kept
    for (u, _) in &posts[2..] {
        assert!(stored.contains(u));
    }
    for (u, _) in &posts[..2] {
        assert!(!stored.contains(u));
    }
    let repo = w.repo(&owner);
    let boundary = posts[2].0.rsplit('/').next().unwrap();
    assert_eq!(
        repo.cap_boundary(&f.fk, nsid::POST),
        Some(boundary),
        "capped: the user card shows [partial]"
    );
    assert_eq!(
        repo.repo_status(&f.fk),
        lessdugin_core::store::RepoStatus::Fresh,
        "a capped range is complete"
    );

    // a byte cap leaves the range incomplete: no listed entry
    let w2 = World::new();
    let owner = w2.net.add_account("owner.example.com");
    let f = w2.forum(&owner, "meta", MembershipMode::Approval, &[]);
    for _ in 0..5 {
        w2.post(&owner, &f);
    }
    let sy = w2.syncer_with(|c| {
        c.tuning.range_max_bytes = Some(10);
        c.tuning.limit = 2;
    });
    let r = w2.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    let repo = w2.repo(&owner);
    assert_eq!(repo.listed_rev(&f.fk), "");
    assert_eq!(repo.byte_capped.get(&f.fk), Some(&true));
    assert_eq!(
        repo.fails, 0,
        "a safety cap is no repo failure (no backoff)"
    );
    assert_eq!(
        repo.repo_status(&f.fk),
        lessdugin_core::store::RepoStatus::Unavailable
    );
}

#[test]
fn rate_limits_back_off_and_succeed() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    w.post(&owner, &f);
    w.net.inject(
        Fault::rate_limited(3)
            .on_host(&owner.pds_base())
            .on_nsid(LIST)
            .once(),
    );
    let sy = w.syncer();
    let t0 = w.host.now();
    let r = w.sync_repo(&sy, &owner, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(w.host.now() - t0 >= 3_000, "waited for Retry-After");
    assert!(
        w.host.sleeps().iter().any(|s| *s >= 3_000),
        "{:?}",
        w.host.sleeps()
    );
    assert_eq!(w.net.requests_for(LIST).len(), 2, "one 429, one retry");
    // a 429 does not count toward the breaker
    let b = sy.net().breaker(&owner.pds_base());
    assert_eq!(b.state, BreakerState::Closed);
    assert_eq!(b.failures, 0);
}

#[test]
fn a_failing_host_opens_its_breaker() {
    let w = World::new();
    let pds2: PdsHandle = w.net.add_pds("https://pds2.example.com");
    let accts: Vec<AccountHandle> = (0..4)
        .map(|i| pds2.add_account(&format!("u{i}.example.com")))
        .collect();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    for a in &accts {
        w.join(a, &f);
    }
    w.net.inject(Fault::status(503).on_host(pds2.base()));
    let sy = w.syncer();
    let mut reports = Vec::new();
    for a in &accts {
        reports.push(w.sync_repo(&sy, a, &f));
    }
    assert!(reports.iter().all(|r| r.outcome == RepoOutcome::Failed));
    let origin = pds2.base().trim_end_matches('/').to_owned();
    let b = sy.net().breaker(&origin);
    assert_eq!(b.state, BreakerState::Open, "{b:?}");
    let sent = w
        .net
        .requests()
        .iter()
        .filter(|r| r.host == origin && r.service == ServiceKind::Pds)
        .count();
    assert_eq!(sent, 10, "nothing is sent once the breaker is open");
    assert!(reports.last().unwrap().refused);
    // repos refused by the breaker get no backoff of their own
    let last = accts.last().unwrap();
    let row = block_on(w.store().repo(last.did().as_str())).unwrap();
    assert!(row.is_none_or(|r| r.fails == 0));
    // the breaker is persisted with the host row
    assert_eq!(host_row(&w, &origin).breaker.state, BreakerState::Open);
}

#[test]
fn point_fetch_and_own_writes() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let p = w.post(&owner, &f);
    let sy = w.syncer();
    let s = w.session(&sy);
    let uri = AtUri::parse(&p.0).unwrap();
    let out = block_on(sy.point_fetch(&s, &uri, None)).unwrap();
    assert!(matches!(
        out,
        lessdugin_core::sync::PointOutcome::Admitted {
            valid: true,
            changed: true,
            ..
        }
    ));
    owner.delete(nsid::POST, uri.rkey().as_str());
    w.host.advance_ms(1); // a later request (equal keys keep the version)
    let out = block_on(sy.point_fetch(&s, &uri, None)).unwrap();
    assert_eq!(
        out,
        lessdugin_core::sync::PointOutcome::NotFound { tombstoned: true }
    );
    assert!(matches!(w.slot(&p.0), Slot::Tomb(_)));

    // an own write: stored with the exact value sent, then confirmed by a listing (P1)
    let (uri2, cid2) = w.post(&owner, &f);
    let (_, rev) = owner.head();
    let value = owner
        .get(nsid::POST, uri2.rsplit('/').next().unwrap())
        .unwrap();
    let commit = lessdugin_atp::xrpc::CommitMeta {
        cid: owner.head().0.to_string(),
        rev: rev.to_string(),
    };
    let applied = block_on(sy.apply_own_writes(
        &s,
        owner.did(),
        w.host.now(),
        Some(&commit),
        &[lessdugin_core::sync::OwnWrite {
            uri: AtUri::parse(&uri2).unwrap(),
            value: Some(value),
            cid: Some(cid2),
        }],
    ))
    .unwrap();
    assert_eq!(applied.stored, vec![uri2.clone()]);
    assert!(is_rec(&w.slot(&uri2)));
    let item = sy.queue_snapshot();
    let item = item.get(owner.did().as_str()).unwrap();
    assert!(item.reasons.contains(&Reason::OwnWrite));
}

#[test]
fn head_checks_queue_changed_repos() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let sy = w.syncer();
    let s = w.session(&sy);
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    assert_eq!(
        block_on(sy.head_check(&s, owner.did(), false)).unwrap(),
        None
    );
    assert_eq!(
        w.repo(&owner).class_at(w.host.now(), false),
        lessdugin_core::store::repo::RepoClass::Hot,
        "the sync just changed rows"
    );
    w.host.advance_ms(31 * 86_400_000);
    w.post(&owner, &f);
    assert_eq!(
        block_on(sy.head_check(&s, owner.did(), false)).unwrap(),
        Some(Priority::P2),
        "a cold repo is queued at P2"
    );
    let item = sy.queue_snapshot();
    assert!(item.get(owner.did().as_str()).is_some());
    // serving the queue syncs it
    w.host.advance_ms(700_000);
    let fk = f.fk.clone();
    let reps = block_on(sy.run_ready(&s, 10, |_| {
        vec![lessdugin_core::sync::Scope::Forum(fk.clone())]
    }));
    assert_eq!(reps.len(), 1);
    assert_eq!(reps[0].outcome, RepoOutcome::Applied);
    assert!(sy.queue_snapshot().is_empty());
}

// ---------------------------------------------------------------------------------------------
// discovery

#[test]
fn approval_mode_syncs_authorities_and_grants() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let admin = w.net.add_account("admin.example.com");
    let alice = w.net.add_account("alice.example.com");
    let bob = w.net.add_account("bob.example.com");
    let carol = w.net.add_account("carol.example.com");
    let dave = w.net.add_account("dave.example.com");
    let f = w.forum(
        &owner,
        "meta",
        MembershipMode::Approval,
        &[(&admin, "admin")],
    );
    for a in [&admin, &alice, &carol, &dave] {
        w.join(a, &f);
    }
    w.access(&owner, &f, alice.did(), AccessLevel::Member);
    w.access(&admin, &f, bob.did(), AccessLevel::Member); // no membership record
    w.access(&owner, &f, dave.did(), AccessLevel::Banned);
    let pa = w.post(&alice, &f);
    w.post(&carol, &f); // an applicant: never synced in approval mode
    w.opinion(&admin, &f, &pa);

    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(
        matches!(fs.forum.proof, DeclProof::Proven { .. }),
        "{:?}",
        fs.forum.proof
    );
    assert_eq!(fs.forum.authorities.len(), 2);
    let r = &fs.state.roster;
    let set = |xs: &[&AccountHandle]| -> BTreeSet<String> {
        xs.iter().map(|a| a.did().to_string()).collect()
    };
    assert_eq!(r.granted, set(&[&alice, &bob]));
    assert_eq!(r.banned, set(&[&dave]));
    assert_eq!(r.seeds, set(&[&owner, &admin, &alice, &bob]));
    assert_eq!(
        fs.state.disc.get(alice.did().as_str()).map(|d| d.state),
        Some(DiscState::Confirmed)
    );
    // bob is granted without a membership: synced like an authority, never rejected
    // (social.md §3.3: the grant counts whatever the membership says)
    assert_eq!(
        fs.state.disc.get(bob.did().as_str()).map(|d| d.state),
        Some(DiscState::Candidate)
    );
    assert!(w.requests_about(bob.did()) > 0, "bob's repo is listed");
    assert!(!fs.state.disc.contains_key(carol.did().as_str()));
    assert_eq!(w.requests_about(carol.did()), 0, "carol is not a candidate");
    assert_eq!(
        w.requests_about(dave.did()),
        0,
        "dave is banned, not granted"
    );
    assert!(w.stored(&f, &alice).contains(&pa.0));
    // C(F) = A ∪ G, bob included
    assert_eq!(fs.completeness.repos_total, 4);
    assert_eq!(fs.completeness.repos_complete, 4);
    assert!(!fs.completeness.partial);
    assert!(!fs.completeness.degraded);
    let rep = &fs.report;
    assert_eq!((rep.repos_total, rep.repos_complete), (4, 4));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.requests as usize, w.net.requests().len());

    // a second sync: the proof is cached, nothing changes
    w.net.clear_requests();
    let fs2 = w.sync_forum(&f, &SyncOpts::default());
    assert!(matches!(fs2.forum.proof, DeclProof::Cached { .. }));
    assert_eq!(fs2.report.changed, 0);
    assert_eq!(w.requests_about(bob.did()), 1, "one head check");
    // warm: the declaration, then one head per synced repo
    assert_eq!(fs2.report.requests, 1 + 4, "{:?}", w.net.requests());
}

#[test]
fn an_unavailable_authority_degrades_the_view() {
    let w = World::new();
    let pds2 = w.net.add_pds("https://pds2.example.com");
    let owner = w.net.add_account("owner.example.com");
    let modr = pds2.add_account("mod.example.com");
    let f = w.forum(
        &owner,
        "meta",
        MembershipMode::Approval,
        &[(&modr, "moderator")],
    );
    w.join(&modr, &f);
    w.net.inject(Fault::disconnect().on_host(pds2.base()));
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(fs.completeness.degraded);
    assert!(fs.completeness.partial);
    assert_eq!(fs.report.repos_unavailable, 1);
    assert!(fs.report.degraded);
    assert!(
        fs.report
            .errors
            .iter()
            .any(|e| e.did.as_deref() == Some(modr.did().as_str()))
    );
    // once listed, a failing authority is only stale: its assertions still apply
    w.net.clear_faults();
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(!fs.completeness.degraded);
    w.post(&modr, &f);
    w.net.inject(Fault::disconnect().on_host(pds2.base()));
    // a new head was observed by a poll before the outage
    let fs = {
        w.net.clear_faults();
        let sy = w.syncer();
        let s = w.session(&sy);
        block_on(sy.head_check(&s, modr.did(), true)).unwrap();
        w.net.inject(Fault::disconnect().on_host(pds2.base()));
        w.sync_forum(&f, &SyncOpts::default())
    };
    assert!(!fs.completeness.degraded, "stale, not unavailable");
    assert_eq!(fs.report.repos_stale, 1);
    assert!(fs.completeness.partial);
}

#[test]
fn an_owner_with_a_hint_identity_cannot_be_opened() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    w.net.inject(Fault::status(500).on_nsid("plc"));
    let e = block_on(w.syncer().sync_forum(f.uri.as_str(), &SyncOpts::default())).unwrap_err();
    assert!(
        matches!(
            e,
            lessdugin_core::discovery::DiscoveryError::Identity(_)
                | lessdugin_core::discovery::DiscoveryError::OwnerHint(_)
        ),
        "{e:?}"
    );
    w.net.clear_faults();
    let missing = ForumKey::new(owner.did(), "nope").unwrap();
    let e = block_on(
        w.syncer()
            .sync_forum(missing.uri.as_str(), &SyncOpts::default()),
    )
    .unwrap_err();
    assert!(matches!(
        e,
        lessdugin_core::discovery::DiscoveryError::NotFound(_)
    ));
}

#[test]
fn open_mode_follows_the_closure_and_newcomers() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let eve = w.net.add_account("eve.example.com");
    let frank = w.net.add_account("frank.example.com");
    let gina = w.net.add_account("gina.example.com");
    let hugo = w.net.add_account("hugo.example.com");
    let f = w.forum(&owner, "open", MembershipMode::Open, &[]);
    for a in [&eve, &gina, &hugo] {
        w.join(a, &f);
    }
    let pe = w.post(&eve, &f);
    let pf = w.post(&frank, &f); // no membership: rejected
    let ph = w.post(&hugo, &f);
    w.post(&gina, &f);
    w.opinion(&owner, &f, &pe);
    w.comment(&owner, &f, &pf);
    w.opinion(&eve, &f, &ph); // eve is not a seed: hugo is not in the closure

    let opts = SyncOpts {
        newcomers: vec![(gina.did().to_string(), "constellation".into())],
        ..SyncOpts::default()
    };
    let fs = w.sync_forum(&f, &opts);
    let dids = |xs: &[&AccountHandle]| -> BTreeSet<String> {
        xs.iter().map(|a| a.did().to_string()).collect()
    };
    assert_eq!(fs.state.closure, dids(&[&eve, &frank]));
    let st = |a: &AccountHandle| fs.state.disc.get(a.did().as_str()).map(|d| d.state);
    assert_eq!(st(&eve), Some(DiscState::Confirmed));
    assert_eq!(st(&frank), Some(DiscState::Rejected));
    assert_eq!(st(&gina), Some(DiscState::Confirmed));
    assert_eq!(st(&hugo), None);
    assert_eq!(
        fs.state.disc[gina.did().as_str()].src,
        vec!["hint:constellation".to_owned()]
    );
    assert!(w.stored(&f, &eve).contains(&pe.0));
    assert!(!w.stored(&f, &frank).contains(&pf.0));
    assert_eq!(w.requests_about(hugo.did()), 0);
    // C(F) = A ∪ S ∪ confirmed closure (newcomers are best effort)
    assert_eq!(fs.completeness.repos_total, 2);
    assert_eq!(fs.completeness.repos_complete, 2);
    assert!(!fs.completeness.partial);
}

#[test]
fn time_and_request_budgets_end_the_command_partial() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let members: Vec<_> = (0..6)
        .map(|i| w.net.add_account(&format!("m{i}.example.com")))
        .collect();
    for m in &members {
        w.join(m, &f);
        w.access(&owner, &f, m.did(), AccessLevel::Member);
    }
    let fs = w.sync_forum(
        &f,
        &SyncOpts {
            max_requests: Some(15),
            ..SyncOpts::default()
        },
    );
    assert_eq!(fs.report.requests, 15);
    assert!(fs.report.partial);
    assert!(fs.report.errors.iter().any(|e| e.code == "request_limit"));
    // refused requests are not repo failures
    for m in &members {
        if let Some(r) = block_on(w.store().repo(m.did().as_str())).unwrap() {
            assert_eq!(r.fails, 0);
        }
    }
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(!fs.report.partial, "{:?}", fs.report);
}

#[test]
fn a_time_budget_also_bounds_waiting() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let members: Vec<_> = (0..4)
        .map(|i| w.net.add_account(&format!("m{i}.example.com")))
        .collect();
    for m in &members {
        w.join(m, &f);
        w.post(m, &f);
        w.access(&owner, &f, m.did(), AccessLevel::Member);
    }
    assert!(!w.sync_forum(&f, &SyncOpts::default()).report.partial);
    // a host asking for 30 s back-offs does not hold a 10 s command longer than 10 s
    for m in &members {
        w.post(m, &f);
    }
    w.net.inject(Fault::rate_limited(30).on_nsid(LIST));
    let t0 = w.host.now();
    let fs = w.sync_forum(
        &f,
        &SyncOpts {
            budget_ms: Some(10_000),
            ..SyncOpts::default()
        },
    );
    assert!(w.host.now() - t0 <= 10_000, "{}", w.host.now() - t0);
    assert!(fs.report.partial);
    // the first listing's 429 holds the whole host back (§9.2); repos whose new head was seen
    // are stale, the others were not even asked
    assert!(fs.report.repos_stale >= 1, "{:?}", fs.report);
    assert!(
        fs.report.errors.iter().any(|e| e.code == "time_budget"),
        "{:?}",
        fs.report.errors
    );
    // nothing was failed or backed off for it
    for m in &members {
        assert_eq!(w.repo(m).fails, 0);
    }
}

/// `sync.md` §14.1: a cold sync costs PLC + 2 heads + describeRepo + one listing per present
/// RANGED collection + the membership `getRecord` per participant: 8 with three collections.
/// (The task's "≤ ~6" is the synthesis's figure, which §14.1 corrects: it omitted the head
/// re-check.) Participants with a single collection cost 6.
#[test]
fn a_cold_sync_of_fifty_members_matches_the_cost_model() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "fifty", MembershipMode::Approval, &[]);
    let mut members = Vec::new();
    for i in 0..50 {
        let m = w.net.add_account(&format!("p{i}.example.com"));
        w.join(&m, &f);
        w.access(&owner, &f, m.did(), AccessLevel::Member);
        members.push(m);
    }
    // half write posts, comments and opinions; half only vote
    let first = w.post(&members[0], &f);
    for (i, m) in members.iter().enumerate() {
        if i % 2 == 0 {
            let p = w.post(m, &f);
            w.comment(m, &f, &first);
            w.opinion(m, &f, &p);
        } else {
            w.opinion(m, &f, &first);
        }
    }
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(!fs.report.partial, "{:?}", fs.report);
    assert_eq!(fs.report.repos_total, 51);
    assert_eq!(fs.report.repos_complete, 51);
    for (i, m) in members.iter().enumerate() {
        let want = if i % 2 == 0 { 8 } else { 6 };
        assert_eq!(w.requests_about(m.did()), want, "participant {i}");
    }
    // the owner: PLC, declaration, proof, 2 heads, describeRepo, one modAction listing (50
    // grants in one page), membership
    assert_eq!(w.requests_about(owner.did()), 8);
    let total = w.net.requests().len();
    assert_eq!(total, 8 + 25 * 8 + 25 * 6);
    assert_eq!(fs.report.requests as usize, total);
    assert!(total <= 51 * 8, "≤ 8 per participant (§14.1)");
    let doh = w
        .net
        .requests()
        .iter()
        .filter(|r| r.service == ServiceKind::Doh)
        .count();
    assert_eq!(doh, 0, "no DoH for participants not on screen");

    // warm: one getLatestCommit per repo, plus the declaration
    w.net.clear_requests();
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert_eq!(fs.report.requests, 1 + 51);
    assert_eq!(fs.report.changed, 0);
}

#[test]
fn engine_sync_forum_returns_the_report() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let handle_ref = format!("{}/meta", owner.handle());
    let rep = block_on(w.engine.sync_forum(&handle_ref, &SyncOpts::default())).unwrap();
    assert_eq!(rep.forum, f.uri.to_string());
    assert_eq!((rep.repos_total, rep.repos_complete), (1, 1));
    let e = block_on(w.engine.sync_forum("not a ref", &SyncOpts::default())).unwrap_err();
    assert_eq!(e.code, lessdugin_core::engine::ErrorCode::InvalidRef);
    assert!(w.host.now() > 0);
}

#[test]
fn recently_active_authors_come_first_within_the_mode() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let ivy = w.net.add_account("ivy.example.com");
    let jon = w.net.add_account("jon.example.com");
    // approval mode: a recently active author who is not granted is never synced
    let f = w.forum(&owner, "appr", MembershipMode::Approval, &[]);
    w.join(&ivy, &f);
    w.join(&jon, &f);
    w.access(&owner, &f, jon.did(), AccessLevel::Member);
    w.post(&ivy, &f);
    let opts = SyncOpts {
        recently_active: vec![ivy.did().to_string(), jon.did().to_string()],
        ..SyncOpts::default()
    };
    let fs = w.sync_forum(&f, &opts);
    assert_eq!(w.requests_about(ivy.did()), 0, "ivy is an applicant");
    assert_eq!(
        fs.state.disc.get(jon.did().as_str()).map(|d| d.state),
        Some(DiscState::Confirmed)
    );
    assert_eq!((fs.report.repos_total, fs.report.repos_complete), (2, 2));

    // open mode: any recently active author is confirmed and synced at once
    let g = w.forum(&owner, "open", MembershipMode::Open, &[]);
    w.join(&ivy, &g);
    let pi = w.post(&ivy, &g);
    let opts = SyncOpts {
        recently_active: vec![ivy.did().to_string()],
        ..SyncOpts::default()
    };
    let fs = w.sync_forum(&g, &opts);
    let row = &fs.state.disc[ivy.did().as_str()];
    assert_eq!(row.state, DiscState::Confirmed);
    assert_eq!(row.src, vec!["hint:active".to_owned()]);
    assert!(w.stored(&g, &ivy).contains(&pi.0));
}

// ---------------------------------------------------------------------------------------------
// scheduler (§9)

/// Counts requests in flight per origin; each request yields to the executor twice before it is
/// answered, so concurrent jobs really overlap.
#[derive(Clone)]
struct CountingHost {
    inner: MemHost,
    counts: Arc<std::sync::Mutex<InFlight>>,
}

#[derive(Default)]
struct InFlight {
    now: std::collections::BTreeMap<String, usize>,
    max: std::collections::BTreeMap<String, usize>,
    global: usize,
    max_global: usize,
}

struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    }
}

impl Host for CountingHost {
    type Kv = <MemHost as Host>::Kv;
    type Ws = <MemHost as Host>::Ws;

    fn cache(&self) -> &Self::Kv {
        self.inner.cache()
    }

    fn state(&self) -> &Self::Kv {
        self.inner.state()
    }

    async fn http(
        &self,
        req: lessdugin_core::host::HttpRequest,
    ) -> Result<lessdugin_core::host::HttpResponse, lessdugin_core::host::HostError> {
        let origin = match &req.target {
            lessdugin_core::host::Target::Anon(u) => u.origin().ascii_serialization(),
            lessdugin_core::host::Target::UserPds { account, .. } => account.to_string(),
        };
        {
            let mut c = self.counts.lock().unwrap();
            let n = c.now.entry(origin.clone()).or_default();
            *n += 1;
            let n = *n;
            let m = c.max.entry(origin.clone()).or_default();
            *m = (*m).max(n);
            c.global += 1;
            c.max_global = c.max_global.max(c.global);
        }
        YieldOnce(false).await;
        YieldOnce(false).await;
        let r = self.inner.http(req).await;
        let mut c = self.counts.lock().unwrap();
        *c.now.get_mut(&origin).unwrap() -= 1;
        c.global -= 1;
        r
    }

    async fn fetch_blob(
        &self,
        pds: url::Url,
        did: Did,
        cid: String,
        max_bytes: u64,
    ) -> Result<lessdugin_core::host::BlobHandle, lessdugin_core::host::HostError> {
        self.inner.fetch_blob(pds, did, cid, max_bytes).await
    }

    async fn websocket(
        &self,
        url: url::Url,
        max_message_bytes: u32,
    ) -> Result<Self::Ws, lessdugin_core::host::HostError> {
        self.inner.websocket(url, max_message_bytes).await
    }

    async fn resolve_txt_system(
        &self,
        name: String,
    ) -> Result<Option<Vec<String>>, lessdugin_core::host::HostError> {
        self.inner.resolve_txt_system(name).await
    }

    async fn sleep(&self, ms: u64) {
        self.inner.sleep(ms).await;
    }

    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }

    fn fill_random(&self, buf: &mut [u8]) {
        self.inner.fill_random(buf);
    }
}

#[test]
fn the_limiter_caps_requests_in_flight() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let pds2 = w.net.add_pds("https://pds2.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let members: Vec<_> = (0..8)
        .map(|i| pds2.add_account(&format!("m{i}.example.com")))
        .collect();
    for m in &members {
        w.join(m, &f);
        w.post(m, &f);
        w.access(&owner, &f, m.did(), AccessLevel::Member);
    }
    let host = CountingHost {
        inner: w.host.clone(),
        counts: Arc::default(),
    };
    let mut config = SyncConfig::from_engine(w.engine.config());
    config.limits = lessdugin_core::sync::NetLimits {
        global: 3,
        per_host: 2,
        plc: 8,
        doh: 8,
    };
    let sy = Syncer::new(Arc::new(host.clone()), config);
    let fs = block_on(sy.sync_forum(f.uri.as_str(), &SyncOpts::default())).unwrap();
    assert!(!fs.report.partial, "{:?}", fs.report);
    assert_eq!(fs.report.repos_complete, 9);
    let c = host.counts.lock().unwrap();
    let origin = pds2.base().trim_end_matches('/');
    assert_eq!(c.max[origin], 2, "per-host cap, reached: {:?}", c.max);
    assert_eq!(c.max_global, 3, "global cap, reached");
    assert!(c.max.values().all(|m| *m <= 2), "{:?}", c.max);
    assert_eq!(c.global, 0);
    assert_eq!(sy.net().inflight(), 0, "every permit released");
}

#[test]
fn read_budgets_hold_background_work_below_half_the_window() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let pds2 = w.net.add_pds("https://pds2.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let members: Vec<_> = (0..4)
        .map(|i| pds2.add_account(&format!("m{i}.example.com")))
        .collect();
    for m in &members {
        w.join(m, &f);
        w.post(m, &f);
        w.access(&owner, &f, m.did(), AccessLevel::Member);
    }
    pds2.set_rate_limit(Some(lessdugin_testkit::fake_pds::RateLimit {
        limit: 20,
        window_ms: 60_000,
    }));
    let t0 = w.host.now();
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(!fs.report.partial, "{:?}", fs.report);
    let origin = pds2.base().trim_end_matches('/').to_owned();
    let sent: Vec<_> = w
        .net
        .requests()
        .into_iter()
        .filter(|r| r.host == origin)
        .collect();
    assert_eq!(
        sent.len(),
        4 * 5,
        "head, describeRepo, posts, membership, head"
    );
    assert!(
        sent.iter().all(|r| r.status != 429),
        "the PDS never had to refuse"
    );
    let first_window = sent.iter().filter(|r| r.at_ms < t0 + 60_000).count();
    assert_eq!(
        first_window, 10,
        "background work uses at most half the window"
    );
    assert!(w.host.now() >= t0 + 60_000, "it waited for the reset");
    let b = w.syncer().net().budget(&origin).unwrap();
    assert_eq!(b.limit, 20);
}

#[test]
fn a_host_without_range_cursors_is_probed_again_after_seven_days() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    w.post(&owner, &f);
    owner.pds().set_cursor_mode(CursorMode::Opaque);
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    let pds = owner.pds_base();
    assert_eq!(host_row(&w, &pds).range_cursor, RangeCursor::No);
    let probe = format!("{}.", f.fk);
    let probes = |w: &World| {
        w.net
            .requests_for(LIST)
            .iter()
            .filter(|l| l.params.get("cursor") == Some(&probe))
            .count()
    };
    // within 7 days: FULL mode, no probe
    owner.pds().set_cursor_mode(CursorMode::Bound);
    w.host.advance_ms(6 * 86_400_000);
    w.post(&owner, &f);
    w.net.clear_requests();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    assert_eq!(probes(&w), 0);
    assert_eq!(host_row(&w, &pds).range_cursor, RangeCursor::No);
    // after 7 days: probed again, and the host now passes
    w.host.advance_ms(86_400_000 + 1);
    w.post(&owner, &f);
    w.net.clear_requests();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    assert_eq!(probes(&w), 1);
    assert_eq!(host_row(&w, &pds).range_cursor, RangeCursor::Yes);
    assert_eq!(
        w.stored(&f, &owner).len(),
        4,
        "three posts and the membership"
    );
}

#[test]
fn a_reconcile_refreshes_collections_even_at_an_unchanged_head() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    assert!(!w.repo(&owner).colls.iter().any(|c| c == nsid::POST));
    // a first post: `repo.colls` (< 24 h old) is reused, so the post collection is not listed
    let p = w.post(&owner, &f);
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    assert!(!is_rec(&w.slot(&p.0)));
    // the reconcile asks describeRepo again and lists the new collection
    let r = w.sync_repo_for(&sy, &owner, &f, Reason::Reconcile);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(is_rec(&w.slot(&p.0)));
    assert!(w.repo(&owner).colls.iter().any(|c| c == nsid::POST));
    // and at an unchanged head with unchanged collections it stays clean
    let r = w.sync_repo_for(&sy, &owner, &f, Reason::Reconcile);
    assert_eq!(r.outcome, RepoOutcome::Clean);
}

// ---------------------------------------------------------------------------------------------
// M2 review regressions (store and sync)

const DESCRIBE: &str = "com.atproto.repo.describeRepo";

fn origin_of(a: &AccountHandle) -> String {
    a.pds_base().trim_end_matches('/').to_owned()
}

/// H1: a granted member who leaves is still a seed (social.md §3.3), so their repo stays synced
/// and counted: a deleted opinion is tombstoned, and a fresh client agrees.
#[test]
fn a_granted_member_who_leaves_stays_synced() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let bob = w.net.add_account("bob.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let p = w.post(&owner, &f);
    w.access(&owner, &f, bob.did(), AccessLevel::Member);
    w.join(&bob, &f);
    w.opinion(&bob, &f, &p);
    let op_rkey = keys::opinion_rkey(NS, f.uri.as_str(), &p.0).unwrap();
    let op = bob.uri(nsid::OPINION, &op_rkey);
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(is_rec(&w.slot(&op)));
    assert_eq!(
        fs.state.disc.get(bob.did().as_str()).map(|d| d.state),
        Some(DiscState::Confirmed)
    );

    // bob leaves, then deletes his opinion
    bob.delete(nsid::MEMBERSHIP, &f.fk);
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(fs.state.roster.seeds.contains(bob.did().as_str()));
    assert_ne!(
        fs.state.disc.get(bob.did().as_str()).map(|d| d.state),
        Some(DiscState::Rejected)
    );
    bob.delete(nsid::OPINION, &op_rkey);
    w.host.advance_ms(2 * 86_400_000);
    let fs = w.sync_forum(&f, &SyncOpts::default());
    assert!(
        matches!(w.slot(&op), Slot::Tomb(_)),
        "the deleted opinion is tombstoned"
    );
    assert!(fs.state.completeness_set().contains(bob.did().as_str()));
    assert_eq!(
        (fs.completeness.repos_total, fs.completeness.partial),
        (2, false)
    );

    // a fresh client of the same network has no opinion either, and counts bob too
    let host2 = MemHost::new();
    host2.set_now_ms(w.host.now());
    w.net.install(&host2);
    let e2 = Engine::new(host2.clone(), EngineConfig::default());
    let fs2 = block_on(e2.syncer().sync_forum(f.uri.as_str(), &SyncOpts::default())).unwrap();
    let k = RecKey::of_uri(&AtUri::parse(&op).unwrap());
    let s2 = block_on(RecordStore::new(host2.cache()).slot(&k)).unwrap();
    assert!(!is_rec(&s2));
    assert_eq!(
        (fs2.completeness.repos_total, fs2.completeness.partial),
        (2, false)
    );
}

/// H2: a `describeRepo` answer without `collections` is a failed `describeRepo` (OURS is listed
/// instead), never "no collections"; and a collection missing from `describeRepo` while rows of
/// it are stored is listed, not wiped.
#[test]
fn describe_repo_never_empties_a_range_without_a_listing() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let posts: Vec<_> = (0..3).map(|_| w.post(&owner, &f)).collect();
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    let describe = |colls: Option<Vec<&str>>| {
        let mut v = json!({
            "handle": "owner.example.com",
            "did": owner.did().as_str(),
            "handleIsCorrect": true,
        });
        if let Some(c) = colls {
            v["collections"] = json!(c);
        }
        Fault::respond(resp::ok_json(&v))
            .on_repo(owner.did().as_str())
            .on_nsid(DESCRIBE)
    };

    w.post(&owner, &f);
    w.net.inject(describe(None));
    w.net.clear_requests();
    let r = w.sync_repo_for(&sy, &owner, &f, Reason::Reconcile);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(
        r.issues.iter().any(|(_, m)| m.contains("collections")),
        "{:?}",
        r.issues
    );
    assert!(w.net.count(LIST) > 0, "OURS is listed instead");
    assert!(posts.iter().all(|p| is_rec(&w.slot(&p.0))));
    let repo = w.repo(&owner);
    assert!(repo.colls.iter().any(|c| c == nsid::POST));
    assert_eq!(
        repo.fails, 0,
        "a failed describeRepo alone is no repo failure"
    );
    assert_eq!(
        repo.repo_status(&f.fk),
        lessdugin_core::store::RepoStatus::Fresh
    );

    // a describeRepo that omits the post collection: its stored range is listed anyway
    w.net.clear_faults();
    w.net
        .inject(describe(Some(vec![nsid::FORUM, nsid::MEMBERSHIP])));
    w.post(&owner, &f);
    w.net.clear_requests();
    let r = w.sync_repo_for(&sy, &owner, &f, Reason::Reconcile);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(
        w.net
            .requests_for(LIST)
            .iter()
            .any(|l| l.params.get("collection").map(String::as_str) == Some(nsid::POST))
    );
    assert!(posts.iter().all(|p| is_rec(&w.slot(&p.0))), "nothing wiped");
}

/// M1: records that fail admission are strikes one by one (`HostRow.strikes`), but a response
/// counts at most one failure toward the host's breaker (R4): one hostile page cannot shut out
/// the other users of a shared PDS.
#[test]
fn bad_records_count_once_per_response_toward_the_breaker() {
    let w = World::new();
    let mallory = w.net.add_account("mallory.example.com");
    let bob = w.net.add_account("bob.example.com");
    let f = w.forum(&mallory, "meta", MembershipMode::Approval, &[]);
    w.post(&mallory, &f);
    w.join(&bob, &f);
    w.post(&bob, &f);
    let recs: Vec<Value> = (0..10)
        .rev()
        .map(|i| {
            let rkey = format!("{}-3l2a2a2a2a2{i:02}", f.fk);
            json!({
                "uri": mallory.uri(nsid::POST, &rkey),
                "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifviqyqutqpnr7tvpya",
                "value": {"$type": nsid::POST, "x": i}
            })
        })
        .collect();
    w.net.inject(
        Fault::respond(resp::ok_json(&json!({ "records": recs })))
            .on_repo(mallory.did().as_str())
            .on_nsid(LIST),
    );
    let sy = w.syncer();
    let r = w.sync_repo(&sy, &mallory, &f);
    assert_eq!(r.strikes, 10, "{r:?}");
    let origin = origin_of(&mallory);
    let b = sy.net().breaker(&origin);
    assert_eq!(b.state, BreakerState::Closed, "{b:?}");
    assert_eq!(b.failures, 1);
    assert_eq!(host_row(&w, &origin).strikes, 10);
    let r = w.sync_repo(&sy, &bob, &f);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
}

/// M2: a response header named like the engine's internal send-time header is dropped, so a
/// server cannot pin `headSeen` (and stop polls) with a far-future send time.
#[test]
fn a_server_cannot_set_the_send_time() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    w.post(&owner, &f);
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    let (cid, rev) = owner.head();
    let body = json!({"cid": cid.to_string(), "rev": rev.to_string()});
    w.net.inject(
        Fault::respond(resp::with_header(
            resp::ok_json(&body),
            "X-Lessdugin-Sent-At",
            "9000000000000000000",
        ))
        .on_repo(owner.did().as_str())
        .on_nsid(HEAD)
        .once(),
    );
    let s = w.session(&sy);
    block_on(sy.head_check(&s, owner.did(), false)).unwrap();
    let seen = w.repo(&owner).head_seen.unwrap();
    assert!(seen.sent_at <= w.host.now(), "{seen:?}");
    // a later head is still taken
    w.host.advance_ms(1_000);
    w.post(&owner, &f);
    let (_, rev2) = owner.head();
    block_on(sy.head_check(&s, owner.did(), false)).unwrap();
    assert_eq!(w.repo(&owner).head_seen.unwrap().rev, rev2.to_string());
}

const SLINGSHOT_MINIDOC: &str =
    "https://slingshot.microcosm.blue/xrpc/com.bad-example.identity.resolveMiniDoc";
const EVIL: &str = "https://evil.example.com";

/// A `getRecord`-shaped entry `{uri, cid, value}` with its true CID.
fn entry(did: &Did, coll: &str, rkey: &str, v: &Value) -> (String, Value) {
    let ipld = lessdugin_atp::datamodel::json_to_ipld(v).unwrap();
    let cid = lessdugin_atp::cid::for_dag_cbor(&lessdugin_atp::dagcbor::encode(&ipld).unwrap());
    let uri = format!("at://{did}/{coll}/{rkey}");
    (
        uri.clone(),
        json!({"uri": uri, "cid": cid.to_string(), "value": v}),
    )
}

/// M3: under an identity hint (a did:web whose document is CORS-blocked, resolved through
/// Slingshot) the PDS is named by no DID document: its rows are stored as idHint with the key
/// `("", sentAt)`, it writes no tombstones, changes no repo state, never replaces a doc-level
/// row and cannot confirm a membership; a doc-level delete supersedes its rows.
#[test]
fn an_identity_hint_never_changes_doc_level_state() {
    use lessdugin_core::host::HostError;
    use lessdugin_core::store::admit::{self, Expect, LexiconValidator};
    use lessdugin_core::store::{Delete, DeleteEvidence, Observation, OrderKey};
    use lessdugin_testkit::FakeIdentity;

    let host = MemHost::new();
    let engine = Engine::new(host.clone(), EngineConfig::default());
    let id = FakeIdentity::web("web.example.com", "web.example.com", EVIL);
    let did = id.did.clone();
    let f = ForumKey::new(&did, "meta").unwrap();
    let now = host.now();
    let at = Datetime::from_millis(now - 3_600_000).unwrap();
    let post = |n: i64, body: &str| {
        let rkey =
            keys::post_rkey(f.uri.as_str(), tid_at_ms(now - 3_600_000 + n).as_ref()).unwrap();
        let v = serde_json::to_value(Post::article(f.uri.clone(), "A title", body, at.clone()))
            .unwrap();
        (rkey, v)
    };
    let (ka, va) = post(1, "Honest text.");
    let (kb, vb) = post(2, "More honest text.");
    let (kc, vc) = post(3, "Only on the hinted PDS.");
    let vm = serde_json::to_value(Membership::new(f.uri.clone(), at.clone())).unwrap();

    // doc-level state from an earlier pass at r1
    let r1 = tid_at_ms(now - 60_000).to_string();
    let store = RecordStore::new(host.cache());
    let mut batch = store.batch();
    let mut doc_cids = Vec::new();
    for (coll, rkey, v) in [
        (nsid::POST, &ka, &va),
        (nsid::POST, &kb, &vb),
        (nsid::MEMBERSHIP, &f.fk, &vm),
    ] {
        let (_, e) = entry(&did, coll, rkey, v);
        let body = serde_json::to_vec(&e).unwrap();
        let expect = Expect {
            did: &did,
            collection: coll,
            rkey: Some(rkey),
        };
        let rec = admit::admit_get_record(&body, &expect, &LexiconValidator).unwrap();
        doc_cids.push(rec.cid);
        let obs = Observation::listing(&r1, now - 60_000, true, now - 60_000);
        block_on(batch.put_version(rec.into_version(obs))).unwrap();
    }
    let head_cid = doc_cids[0];
    let mut repo = RepoRow::new("https://honest.example.com");
    repo.observe_head(&r1, head_cid, now - 60_000);
    repo.apply_stable_pass(
        &r1,
        head_cid,
        [(f.fk.as_str(), std::collections::BTreeMap::new())],
    );
    repo.colls = vec![
        nsid::FORUM.to_owned(),
        nsid::MEMBERSHIP.to_owned(),
        nsid::POST.to_owned(),
    ];
    repo.colls_at = now - 60_000;
    batch.put_repo(did.as_str(), &repo).unwrap();
    block_on(batch.commit()).unwrap();

    // the document is CORS-blocked; Slingshot names a hostile PDS that serves a far-future
    // head, another version of `a`, a new `c`, and no `b` and no membership
    host.fail(
        "https://web.example.com/.well-known/did.json",
        HostError::Cors,
    );
    host.respond_get(
        SLINGSHOT_MINIDOC,
        resp::ok_json(&json!({
            "did": did.as_str(),
            "handle": "web.example.com",
            "pds": EVIL,
            "signing_key": id.multikey(),
        })),
    );
    let future = tid_at_ms(now + 10 * 365 * 86_400_000).to_string();
    host.respond_get(
        &format!("{EVIL}/xrpc/{HEAD}"),
        resp::ok_json(&json!({"cid": head_cid.to_string(), "rev": future})),
    );
    host.respond_get(
        &format!("{EVIL}/xrpc/{DESCRIBE}"),
        resp::ok_json(&json!({
            "did": did.as_str(),
            "handle": "web.example.com",
            "handleIsCorrect": true,
            "collections": [nsid::POST],
        })),
    );
    let (_, ea) = entry(&did, nsid::POST, &ka, &json!({"$type": nsid::POST, "x": 1}));
    let (uc, ec) = entry(&did, nsid::POST, &kc, &vc);
    let mut page = vec![(ka.clone(), ea), (kc.clone(), ec)];
    page.sort_by(|a, b| b.0.cmp(&a.0));
    let records: Vec<Value> = page.into_iter().map(|(_, e)| e).collect();
    host.respond_get(
        &format!("{EVIL}/xrpc/{LIST}"),
        resp::ok_json(&json!({ "records": records })),
    );
    host.respond_get(
        &format!("{EVIL}/xrpc/com.atproto.repo.getRecord"),
        resp::xrpc_error(400, "RecordNotFound", "no such record"),
    );

    let sy = engine.syncer();
    let s = sy.session(Priority::P1, Arc::new(Stats::new()));
    let rep = block_on(sy.sync_repo(
        &s,
        &SyncRequest::forum(did.clone(), &f.fk, Reason::Participant),
    ));
    assert!(rep.id_hint, "{rep:?}");
    assert_eq!(rep.outcome, RepoOutcome::Applied, "{rep:?}");
    let uri = |coll: &str, rkey: &str| format!("at://{did}/{coll}/{rkey}");
    let slot = |u: &str| block_on(store.slot(&RecKey::of_uri(&AtUri::parse(u).unwrap()))).unwrap();
    // doc-level rows: untouched, none tombstoned
    let Slot::Rec(a) = slot(&uri(nsid::POST, &ka)) else {
        panic!("a is gone")
    };
    assert_eq!(
        a.cid, doc_cids[0],
        "a hint version never replaces a doc-level one"
    );
    assert!(!a.id_hint);
    assert!(
        is_rec(&slot(&uri(nsid::POST, &kb))),
        "no tombstone from a hint"
    );
    assert!(is_rec(&slot(&uri(nsid::MEMBERSHIP, &f.fk))));
    // the new row is a hint row, keyed below every doc-level key
    let Slot::Rec(c) = slot(&uc) else {
        panic!("c not stored")
    };
    assert!(c.id_hint && !c.exact);
    assert_eq!(c.rev_lo, "");
    // no repo state from the hint
    let after = block_on(store.repo(did.as_str())).unwrap().unwrap();
    assert_eq!(after.rev, r1);
    assert_eq!(after.listed_rev(&f.fk), r1);
    assert_eq!(after.head_seen.as_ref().unwrap().rev, r1);
    assert_eq!(after.colls, repo.colls);
    assert_eq!(after.pds, repo.pds);

    // a membership cannot be confirmed under a hint: the candidate stays a candidate
    let rep = block_on(
        sy.sync_repo(
            &s,
            &SyncRequest::forum(did.clone(), &f.fk, Reason::Participant)
                .confirming(f.uri.clone(), &f.fk),
        ),
    );
    assert_eq!(rep.outcome, RepoOutcome::Failed);
    assert!(
        matches!(
            rep.error,
            Some(lessdugin_core::sync::SyncError::IdentityHint { .. })
        ),
        "{rep:?}"
    );

    // a doc-level delete supersedes the hint row
    let mut batch = store.batch();
    let d = Delete {
        key: OrderKey::new(&r1, now),
        evidence: DeleteEvidence::CompleteListing,
        at_ms: now,
    };
    block_on(batch.tombstone(&AtUri::parse(&uc).unwrap(), d)).unwrap();
    block_on(batch.commit()).unwrap();
    assert!(matches!(slot(&uc), Slot::Tomb(_)));
}

/// L1: `sync --full` after a rollback forgets the DID's rows in every scope, not only the
/// requested one, and the scopes it does not list read `Unavailable` until they are listed.
#[test]
fn a_full_sync_after_a_rollback_forgets_every_scope() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f1 = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let f2 = w.forum(&owner, "other", MembershipMode::Approval, &[]);
    let p1 = w.post(&owner, &f1);
    let p2 = w.post(&owner, &f2);
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f1).outcome, RepoOutcome::Applied);
    assert_eq!(w.sync_repo(&sy, &owner, &f2).outcome, RepoOutcome::Applied);
    assert!(is_rec(&w.slot(&p2.0)));
    owner.delete(nsid::POST, p1.0.rsplit('/').next().unwrap());
    owner.delete(nsid::POST, p2.0.rsplit('/').next().unwrap());
    owner.commit_at(tid_at_ms(w.host.now() - 86_400_000));
    let r = w.sync_repo_for(&sy, &owner, &f1, Reason::Full);
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(!is_rec(&w.slot(&p1.0)));
    assert!(
        matches!(w.slot(&p2.0), Slot::Empty),
        "the other forum's rows are forgotten too"
    );
    let repo = w.repo(&owner);
    assert_eq!(
        repo.repo_status(&f1.fk),
        lessdugin_core::store::RepoStatus::Fresh
    );
    assert_eq!(
        repo.repo_status(&f2.fk),
        lessdugin_core::store::RepoStatus::Unavailable
    );
}

/// L2: a row below the cap boundary a listing recorded (a point fetch beyond the fetch cap)
/// never enters the model, even though fewer than N rows are stored.
#[test]
fn rows_below_a_cap_boundary_stay_out_of_the_model() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let mut posts: Vec<_> = (0..5).map(|_| w.post(&owner, &f)).collect();
    posts.sort();
    let sy = w.syncer_with(|c| c.tuning.fetch_cap = Some(3));
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    let s = w.session(&sy);
    for u in [&posts[0].0, &f.uri.to_string()] {
        block_on(sy.point_fetch(&s, &AtUri::parse(u).unwrap(), None)).unwrap();
    }
    assert!(is_rec(&w.slot(&posts[0].0)), "stored by the point fetch");
    let m = block_on(lessdugin_core::model::ForumModel::build(
        w.host.cache(),
        &f,
        w.host.now() * 1000,
    ))
    .unwrap();
    assert!(!m.rows.contains_key(&posts[0].0), "below the boundary");
    for (u, _) in &posts[2..] {
        assert!(m.rows.contains_key(u));
    }
}

/// L3: a pass merges what it learned about its host into the stored row inside its batch,
/// instead of writing back the copy it loaded at its start (which would lose what concurrent
/// passes and the outbox stored meanwhile).
#[test]
fn a_pass_merges_its_host_row() {
    use futures::FutureExt;
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    w.post(&owner, &f);
    let origin = origin_of(&owner);
    let h = w.host.clone();
    let o2 = origin.clone();
    let did = owner.did().to_string();
    // another writer updates the host row while the pass runs (here: during describeRepo)
    w.host
        .on_get(&format!("{origin}/xrpc/{DESCRIBE}"), move |_| {
            let store = RecordStore::new(h.cache());
            let mut b = store.batch();
            let mut row = b
                .host_caps(&o2)
                .now_or_never()
                .unwrap()
                .unwrap()
                .unwrap_or_default();
            row.null_swap = Some(false);
            row.strikes += 7;
            b.put_host_caps(&o2, &row).unwrap();
            b.commit().now_or_never().unwrap().unwrap();
            Ok(resp::ok_json(&json!({
                "did": did,
                "handle": "owner.example.com",
                "handleIsCorrect": true,
                "collections": [nsid::FORUM, nsid::MEMBERSHIP, nsid::POST],
            })))
        });
    let sy = w.syncer();
    assert_eq!(w.sync_repo(&sy, &owner, &f).outcome, RepoOutcome::Applied);
    let row = host_row(&w, &origin);
    assert_eq!(row.null_swap, Some(false));
    assert_eq!(row.strikes, 7);
    assert_eq!(
        row.range_cursor,
        RangeCursor::Yes,
        "and the pass's own verdict"
    );
}

/// L7: an own write without a commit (a no-op) is keyed by a later head's rev, so it is stored
/// as a lower bound (not exact).
#[test]
fn a_no_op_own_write_is_not_exact() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let (uri, cid) = w.post(&owner, &f);
    let value = owner
        .get(nsid::POST, uri.rsplit('/').next().unwrap())
        .unwrap();
    let sy = w.syncer();
    let s = w.session(&sy);
    let applied = block_on(sy.apply_own_writes(
        &s,
        owner.did(),
        w.host.now(),
        None,
        &[lessdugin_core::sync::OwnWrite {
            uri: AtUri::parse(&uri).unwrap(),
            value: Some(value),
            cid: Some(cid),
        }],
    ))
    .unwrap();
    assert_eq!(applied.stored, vec![uri.clone()]);
    let Slot::Rec(r) = w.slot(&uri) else {
        panic!("not stored")
    };
    assert!(!r.exact);
}

/// L9: a complete, stable full listing of the own repo tombstones the stored rows it lacks
/// (deletes made on another device).
#[test]
fn an_own_listing_tombstones_records_deleted_elsewhere() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[]);
    let p = w.post(&owner, &f);
    let keep = w.post(&owner, &f);
    let sy = w.syncer();
    let s = w.session(&sy);
    let mut req = SyncRequest::forum(owner.did().clone(), &f.fk, Reason::Participant);
    req.scopes = vec![lessdugin_core::sync::Scope::Own];
    let r = block_on(sy.sync_repo(&s, &req));
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(is_rec(&w.slot(&p.0)));
    owner.delete(nsid::POST, p.0.rsplit('/').next().unwrap());
    let r = block_on(sy.sync_repo(&s, &req));
    assert_eq!(r.outcome, RepoOutcome::Applied, "{r:?}");
    assert!(matches!(w.slot(&p.0), Slot::Tomb(_)));
    assert!(is_rec(&w.slot(&keep.0)));
    assert!(is_rec(&w.slot(f.uri.as_str())), "the declaration is listed");
}
