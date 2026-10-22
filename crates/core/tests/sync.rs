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
use lessdugin_core::store::repo::RangeCursor;
use lessdugin_core::store::{RecKey, RecordStore, RepoRow, Slot};
use lessdugin_core::sync::net::BreakerState;
use lessdugin_core::sync::{
    Priority, Reason, RepoOutcome, RepoReport, Session, Stats, SyncConfig, SyncRequest, Syncer,
};
use lessdugin_lexicon::keys;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::ns::NS;
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
        self.put(a, nsid::MEMBERSHIP, &f.fk, &serde_json::to_value(&m).unwrap());
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
        block_on(self.store().repo(a.did().as_str())).unwrap().unwrap()
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
    assert_eq!(host_row(&w, &owner.pds_base()).range_cursor, RangeCursor::Yes);
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
    assert!(matches!(w.slot(&p.0), Slot::Tomb(_)));
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
        Fault::respond(resp::ok_json(&json!({"cid": cid.to_string(), "rev": moved})))
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
    assert!(w.host.sleeps().iter().any(|s| *s >= 3_000), "{:?}", w.host.sleeps());
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
    w.net.inject(Fault::status(503).on_host(&pds2.base().to_owned()));
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
        lessdugin_core::sync::PointOutcome::Admitted { valid: true, changed: true, .. }
    ));
    owner.delete(nsid::POST, uri.rkey().as_str());
    let out = block_on(sy.point_fetch(&s, &uri, None)).unwrap();
    assert_eq!(
        out,
        lessdugin_core::sync::PointOutcome::NotFound { tombstoned: true }
    );
    assert!(matches!(w.slot(&p.0), Slot::Tomb(_)));

    // an own write: stored with the exact value sent, then confirmed by a listing (P1)
    let (uri2, cid2) = w.post(&owner, &f);
    let (_, rev) = owner.head();
    let value = owner.get(nsid::POST, uri2.rsplit('/').next().unwrap()).unwrap();
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
fn approval_mode_syncs_authorities_and_confirmed_grants() {
    let w = World::new();
    let owner = w.net.add_account("owner.example.com");
    let admin = w.net.add_account("admin.example.com");
    let alice = w.net.add_account("alice.example.com");
    let bob = w.net.add_account("bob.example.com");
    let carol = w.net.add_account("carol.example.com");
    let dave = w.net.add_account("dave.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[(&admin, "admin")]);
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
    assert!(matches!(fs.forum.proof, DeclProof::Proven { .. }), "{:?}", fs.forum.proof);
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
    assert_eq!(
        fs.state.disc.get(bob.did().as_str()).map(|d| d.state),
        Some(DiscState::Rejected)
    );
    assert!(!fs.state.disc.contains_key(carol.did().as_str()));
    assert_eq!(w.requests_about(carol.did()), 0, "carol is not a candidate");
    assert_eq!(w.requests_about(dave.did()), 0, "dave is banned, not granted");
    assert!(w.stored(&f, &alice).contains(&pa.0));
    // C(F) = A ∪ G without the rejected bob
    assert_eq!(fs.completeness.repos_total, 3);
    assert_eq!(fs.completeness.repos_complete, 3);
    assert!(!fs.completeness.partial);
    assert!(!fs.completeness.degraded);
    let rep = &fs.report;
    assert_eq!((rep.repos_total, rep.repos_complete), (3, 3));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.requests as usize, w.net.requests().len());

    // a second sync: the proof is cached, bob is not re-checked, nothing changes
    w.net.clear_requests();
    let fs2 = w.sync_forum(&f, &SyncOpts::default());
    assert!(matches!(fs2.forum.proof, DeclProof::Cached { .. }));
    assert_eq!(fs2.report.changed, 0);
    assert_eq!(w.requests_about(bob.did()), 0);
    // warm: the declaration, then one head per synced repo
    assert_eq!(fs2.report.requests, 1 + 3, "{:?}", w.net.requests());
}

#[test]
fn an_unavailable_authority_degrades_the_view() {
    let w = World::new();
    let pds2 = w.net.add_pds("https://pds2.example.com");
    let owner = w.net.add_account("owner.example.com");
    let modr = pds2.add_account("mod.example.com");
    let f = w.forum(&owner, "meta", MembershipMode::Approval, &[(&modr, "moderator")]);
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
    let e = block_on(w.syncer().sync_forum(missing.uri.as_str(), &SyncOpts::default()))
        .unwrap_err();
    assert!(matches!(e, lessdugin_core::discovery::DiscoveryError::NotFound(_)));
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
