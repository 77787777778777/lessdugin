// SPDX-License-Identifier: AGPL-3.0-or-later
//! M3 engine actions end to end over the fake network (`MemHost` + `FakePds`): opinions (the
//! undo window, coalescing, two-device merge, swap retries, delete when empty, the checks
//! before enqueueing), moderation writes (hide, keep, removal, the lock allow-list and its
//! 5,000 refusal, author scope, `mods add`/`mods rm` adopting first, `forum_update`, approve and
//! reject), notifications from first-seen times, read state and the local lists.
//!
//! alice owns an approval-mode forum and grants bob and carol (so they are seeds, hence
//! established); dave is a member who was never granted. A repo synced moments ago is not
//! refetched, so every re-sync first moves the clocks ([`World::resync`]).

mod common;

use std::sync::{Arc, Mutex};

use common::block_on;
use lessdugin_core::dto::{
    AccessLevel, ActionTicket, AgreementValue, Capability, CommentDraft, Event, ForumDraft,
    KarmaCadence, KarmaValue, MembershipMode, ModActionIntent, ModActionValue, ModRole, ModScope,
    ModsChange, NoticeCode, NotificationReason, NotificationsQuery, OpinionIntent, PinScope,
    PostDraft, PublicActionKind, Stance, Subscription, SyncOpts, TextQuote, Tier, Topic,
    WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_core::outbox::records;
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::{AccountHandle, Fault};
use lessdugin_testkit::repo::tid_at_ms;
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::{Value, json};

const MINUTE: i64 = 60_000;

struct User {
    host: MemHost,
    engine: Engine<MemHost>,
    acct: AccountHandle,
    events: Arc<Mutex<Vec<Event>>>,
}

impl User {
    fn new(net: &FakeNet, host: MemHost, acct: AccountHandle) -> Self {
        let engine = Engine::new(host.clone(), EngineConfig::default());
        block_on(engine.add_account(AccountInfo::app_password(
            acct.did().as_str(),
            acct.pds_base(),
        )))
        .unwrap();
        let _ = net;
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = Arc::clone(&events);
        engine.subscribe(move |e| sink.lock().unwrap().push(e));
        Self {
            host,
            engine,
            acct,
            events,
        }
    }

    fn did(&self) -> String {
        self.acct.did().to_string()
    }

    /// Runs the outbox until idle; every write must be done.
    fn flush(&self) {
        let r = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!(
            (r.failed, r.conflicts),
            (0, 0),
            "{}: {:?}",
            self.acct.handle(),
            block_on(self.engine.writes()).unwrap()
        );
    }

    fn state(&self, id: &str) -> WriteState {
        block_on(self.engine.write_status(id))
            .unwrap()
            .unwrap()
            .state
    }

    /// The uri a done write created.
    fn done(&self, t: &WriteTicket) -> String {
        match self.state(&t.id) {
            WriteState::Done { uri, .. } => uri,
            other => panic!("write {} is {other:?}", t.id),
        }
    }

    fn sync(&self, forum: &str) {
        let r = block_on(self.engine.sync_forum(forum, &SyncOpts::default())).unwrap();
        assert!(!r.partial, "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
    }

    fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    fn post(&self, forum: &str, title: &str, body: &str) -> String {
        let t = block_on(self.engine.publish(&PostDraft::article(forum, title, body))).unwrap();
        self.flush();
        self.done(&t)
    }

    fn comment(&self, root: &str, parent: Option<&str>, body: &str) -> String {
        let t = block_on(self.engine.comment(&CommentDraft::new(
            root,
            parent.map(str::to_owned),
            body,
        )))
        .unwrap();
        self.flush();
        self.done(&t)
    }

    fn opinion(&self, subject: &str, intent: OpinionIntent) -> ActionTicket {
        block_on(self.engine.opinion(subject, &intent)).unwrap()
    }

    fn opinion_err(&self, subject: &str, intent: OpinionIntent) -> (ErrorCode, String) {
        let e = block_on(self.engine.opinion(subject, &intent)).unwrap_err();
        (e.code, e.message)
    }

    fn mod_action(&self, intent: ModActionIntent) -> ActionTicket {
        block_on(self.engine.mod_action(&intent)).unwrap()
    }

    fn mod_err(&self, intent: ModActionIntent) -> lessdugin_core::engine::EngineError {
        block_on(self.engine.mod_action(&intent)).unwrap_err()
    }

    /// The account's own record, from its PDS.
    fn record(&self, collection: &str, rkey: &str) -> Option<Value> {
        self.acct.get(collection, rkey)
    }
}

struct World {
    net: FakeNet,
    alice: User,
    bob: User,
    carol: User,
    dave: User,
    forum: String,
    /// alice's welcome post.
    welcome: String,
    /// bob's post.
    bob_post: String,
}

fn up() -> OpinionIntent {
    OpinionIntent::Karma {
        value: Some(KarmaValue::Up),
    }
}

fn agree() -> OpinionIntent {
    OpinionIntent::Agreement {
        value: Some(AgreementValue::Agree),
    }
}

fn react(id: &str, stance: Stance) -> OpinionIntent {
    OpinionIntent::React {
        id: id.into(),
        stance,
        quote: None,
        remove: false,
    }
}

fn assert_mod(forum: &str, subject: &str, action: ModActionValue) -> ModActionIntent {
    ModActionIntent::Assert {
        forum: forum.into(),
        scope: ModScope::Forum,
        subject: subject.into(),
        action,
        reason: None,
        until_ms: None,
    }
}

fn hide() -> ModActionValue {
    ModActionValue::Hide {
        hidden: true,
        spam: false,
        show_reason: true,
    }
}

impl World {
    fn new() -> Self {
        Self::with_mode(MembershipMode::Approval)
    }

    fn with_mode(mode: MembershipMode) -> Self {
        let ha = MemHost::new();
        let net = FakeNet::on(&ha);
        let _ = net.default_pds();
        let mut users = Vec::new();
        for (i, name) in ["bob", "carol", "dave"].iter().enumerate() {
            let h = MemHost::new();
            h.set_seed(2 + i as u64);
            net.install_as(&h, name);
            users.push(h);
        }
        let alice = User::new(&net, ha, net.add_account("alice.example.com"));
        let dave_h = users.pop().unwrap();
        let carol_h = users.pop().unwrap();
        let bob_h = users.pop().unwrap();
        let bob = User::new(&net, bob_h, net.add_account("bob.example.com"));
        let carol = User::new(&net, carol_h, net.add_account("carol.example.com"));
        let dave = User::new(&net, dave_h, net.add_account("dave.example.com"));

        let mut draft = ForumDraft::new("meta", "Meta");
        draft.membership = mode;
        let t = block_on(alice.engine.forum_create(&draft)).unwrap();
        alice.flush();
        let forum = alice.done(&t);
        if mode == MembershipMode::Approval {
            block_on(alice.engine.grant(&forum, &[bob.did(), carol.did()])).unwrap();
            alice.flush();
        }
        let mut w = Self {
            net,
            alice,
            bob,
            carol,
            dave,
            forum,
            welcome: String::new(),
            bob_post: String::new(),
        };
        w.tick(MINUTE);
        w.welcome = w.alice.post(&w.forum, "Welcome", "Hello **everyone**.");
        w.tick(MINUTE);
        for u in [&w.bob, &w.carol, &w.dave] {
            block_on(u.engine.forum_open(&w.forum)).unwrap();
            block_on(u.engine.join(&w.forum, None)).unwrap();
            u.flush();
        }
        w.tick(MINUTE);
        w.bob_post = w.bob.post(
            &w.forum,
            "Hello from bob",
            "The cat sat on the mat. Read the rules.",
        );
        w.sync_all();
        w
    }

    fn users(&self) -> [&User; 4] {
        [&self.alice, &self.bob, &self.carol, &self.dave]
    }

    fn tick(&self, ms: i64) {
        for u in self.users() {
            u.host.advance_ms(ms);
        }
    }

    /// Moves every clock a minute (a repo synced moments ago is not refetched) and syncs
    /// everyone.
    fn sync_all(&self) {
        self.resync(&self.users());
    }

    /// Moves every clock a minute and syncs `us`.
    fn resync(&self, us: &[&User]) {
        self.tick(MINUTE);
        for u in us {
            u.sync(&self.forum);
        }
    }

    /// Replaces the forum's gates in alice's declaration (the forum-create preset is a small
    /// forum's: reactCreate 0, reactSecond 0, reactAnti 5, authorModPersonal 10).
    fn set_gates(&self, gates: Value) {
        let mut decl = self.alice.record(nsid::FORUM, "meta").unwrap();
        decl["policy"]["gates"] = gates;
        self.alice.acct.put(nsid::FORUM, "meta", &decl);
    }

    fn opinion_rkey(&self, subject: &str) -> String {
        records::opinion_rkey(&self.forum, subject).unwrap()
    }
}

// ---------------------------------------------------------------------------------------------
// Opinions

#[test]
fn opinion_is_held_and_undone_inside_the_window() {
    let w = World::new();
    let carol = &w.carol;
    let puts = w.net.count("com.atproto.repo.putRecord");
    let t = carol.opinion(&w.bob_post, up());
    assert!(t.notices.is_empty(), "carol is a seed: {t:?}");
    match &t.state {
        WriteState::Pending {
            held: true,
            not_before_ms: Some(nb),
            ..
        } => assert_eq!(*nb, carol.host.now() + 4_000),
        other => panic!("{other:?}"),
    }
    // undo within the 4 s window: removed, never sent
    carol.host.advance_ms(3_000);
    let u = block_on(carol.engine.undo(&t.id)).unwrap();
    assert_eq!(u.state, WriteState::Undone);
    carol.flush();
    assert_eq!(
        carol.record(nsid::OPINION, &w.opinion_rkey(&w.bob_post)),
        None
    );
    assert_eq!(
        w.net.count("com.atproto.repo.putRecord"),
        puts,
        "nothing was sent"
    );
}

#[test]
fn intents_on_one_subject_coalesce_into_one_write() {
    let w = World::new();
    let carol = &w.carol;
    let puts = w.net.count("com.atproto.repo.putRecord");
    let a = carol.opinion(&w.bob_post, up());
    carol.host.advance_ms(2_000);
    let b = carol.opinion(&w.bob_post, agree());
    let c = carol.opinion(
        &w.bob_post,
        OpinionIntent::Karma {
            value: Some(KarmaValue::StrongUp),
        },
    );
    assert_eq!(a.id, b.id);
    assert_eq!(b.id, c.id);
    // the hold restarted with the last intent
    match &c.state {
        WriteState::Pending {
            not_before_ms: Some(nb),
            ..
        } => assert_eq!(*nb, carol.host.now() + 4_000),
        other => panic!("{other:?}"),
    }
    carol.flush();
    let rec = carol
        .record(nsid::OPINION, &w.opinion_rkey(&w.bob_post))
        .unwrap();
    assert_eq!(rec["karma"], "strongUp");
    assert_eq!(rec["agreement"], "agree");
    assert_eq!(rec["subject"]["uri"], w.bob_post.as_str());
    assert_eq!(rec["forum"], w.forum.as_str());
    // one write, one request
    assert_eq!(w.net.count("com.atproto.repo.putRecord") - puts, 1);
}

#[test]
fn opinions_from_two_devices_merge_per_axis() {
    let w = World::new();
    // carol's second device
    let h2 = MemHost::new();
    h2.set_seed(9);
    w.net.install_as(&h2, "carol2");
    h2.set_now_ms(w.carol.host.now());
    let carol2 = User::new(&w.net, h2, w.carol.acct.clone());
    carol2.sync(&w.forum);

    let rkey = w.opinion_rkey(&w.bob_post);
    w.carol.opinion(&w.bob_post, up());
    carol2.opinion(&w.bob_post, agree());
    w.carol.flush();
    assert_eq!(w.carol.record(nsid::OPINION, &rkey).unwrap()["karma"], "up");
    // the second device re-reads the record and re-applies its own axis
    carol2.flush();
    let rec = w.carol.record(nsid::OPINION, &rkey).unwrap();
    assert_eq!(
        (rec["karma"].as_str(), rec["agreement"].as_str()),
        (Some("up"), Some("agree"))
    );

    // a swap failure (another device wrote in between): re-read, re-apply, done
    let fault = w.net.inject(
        Fault::error(400, "InvalidSwap", "record changed")
            .on_nsid("com.atproto.repo.putRecord")
            .once(),
    );
    let before = w.net.count("com.atproto.repo.putRecord");
    carol2.opinion(
        &w.bob_post,
        OpinionIntent::Agreement {
            value: Some(AgreementValue::StrongDisagree),
        },
    );
    carol2.flush();
    assert_eq!(w.net.fault_hits(fault), None, "applied once, then expired");
    assert_eq!(w.net.count("com.atproto.repo.putRecord") - before, 2);
    let rec = w.carol.record(nsid::OPINION, &rkey).unwrap();
    assert_eq!(rec["karma"], "up", "the other axis is kept");
    assert_eq!(rec["agreement"], "strongDisagree");

    // clearing every axis deletes the record
    w.carol
        .opinion(&w.bob_post, OpinionIntent::Karma { value: None });
    carol2.opinion(&w.bob_post, OpinionIntent::Agreement { value: None });
    w.carol.flush();
    carol2.flush();
    assert_eq!(w.carol.record(nsid::OPINION, &rkey), None);
}

#[test]
fn undo_after_sending_restores_the_prior_opinion() {
    let w = World::new();
    let carol = &w.carol;
    let rkey = w.opinion_rkey(&w.bob_post);
    carol.opinion(&w.bob_post, agree());
    carol.flush();
    let t = carol.opinion(&w.bob_post, up());
    carol.flush();
    assert_eq!(carol.record(nsid::OPINION, &rkey).unwrap()["karma"], "up");
    let inv = block_on(carol.engine.undo(&t.id)).unwrap();
    assert_ne!(inv.id, t.id, "an inverse write");
    carol.flush();
    let rec = carol.record(nsid::OPINION, &rkey).unwrap();
    assert_eq!(rec.get("karma"), None);
    assert_eq!(rec["agreement"], "agree");
}

#[test]
fn react_quotes_get_their_selector_from_the_item_text() {
    let w = World::new();
    let alice = &w.alice;
    let t = alice.opinion(
        &w.bob_post,
        OpinionIntent::React {
            id: "insightful".into(),
            stance: Stance::For,
            quote: Some(TextQuote {
                exact: "sat on".into(),
                prefix: None,
                suffix: None,
                start: None,
            }),
            remove: false,
        },
    );
    assert!(t.notices.is_empty());
    alice.flush();
    let rec = alice
        .record(nsid::OPINION, &w.opinion_rkey(&w.bob_post))
        .unwrap();
    let q = &rec["reacts"][0]["quote"];
    assert_eq!(q["exact"], "sat on");
    assert_eq!(q["prefix"], "The cat ");
    assert_eq!(q["start"], 8);
    // a quote that is not in the item is refused
    let (code, msg) = alice.opinion_err(
        &w.bob_post,
        OpinionIntent::React {
            id: "typo".into(),
            stance: Stance::For,
            quote: Some(TextQuote {
                exact: "dog".into(),
                prefix: None,
                suffix: None,
                start: None,
            }),
            remove: false,
        },
    );
    assert_eq!(code, ErrorCode::InvalidInput, "{msg}");
}

#[test]
fn opinion_checks_refuse_what_could_never_count() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    // own items
    let (code, msg) = bob.opinion_err(&w.bob_post, up());
    assert_eq!(code, ErrorCode::InvalidInput, "{msg}");
    // nothing to react against yet
    let (code, _) = carol.opinion_err(&w.welcome, react("insightful", Stance::Against));
    assert_eq!(code, ErrorCode::InvalidInput);
    // not in the palette
    let (code, _) = alice.opinion_err(&w.bob_post, react("not-a-react", Stance::For));
    assert_eq!(code, ErrorCode::InvalidInput);
    // the preset's gates: anyone starts or seconds a react; opposing one needs 5
    carol.opinion(&w.welcome, react("insightful", Stance::For));
    carol.flush();
    w.resync(&[bob]);
    let e = block_on(
        bob.engine
            .opinion(&w.welcome, &react("insightful", Stance::Against)),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Blocked);
    assert_eq!(
        e.message,
        "reacting against needs 5 karma in this forum (you have 0)"
    );
    let d = e.detail.unwrap();
    assert_eq!(
        (d["gate"].as_str(), d["need"].as_i64()),
        (Some("reactAnti"), Some(5))
    );
    // stricter gates: starting a react needs 10, seconding one 5; authorities pass every gate
    w.set_gates(json!({"reactCreate": 10, "reactSecond": 5, "reactAnti": 20}));
    w.sync_all();
    let t = alice.opinion(&w.bob_post, react("insightful", Stance::For));
    assert!(t.notices.is_empty());
    alice.flush();
    w.resync(&[carol]);
    let (code, msg) = carol.opinion_err(&w.bob_post, react("typo", Stance::For));
    assert_eq!(code, ErrorCode::Blocked);
    assert!(
        msg.starts_with("starting a new react needs 10 karma"),
        "{msg}"
    );
    let (code, msg) = carol.opinion_err(&w.bob_post, react("insightful", Stance::For));
    assert_eq!(code, ErrorCode::Blocked);
    assert!(msg.starts_with("seconding a react needs 5 karma"), "{msg}");
    // carol's own react on the welcome post no longer passes reactCreate (gates apply at fold
    // time), so it is gone from the tallies; removing it is always allowed
    let (code, _) = carol.opinion_err(&w.welcome, react("insightful", Stance::For));
    assert_eq!(code, ErrorCode::Blocked);
    carol.opinion(
        &w.welcome,
        OpinionIntent::React {
            id: "insightful".into(),
            stance: Stance::For,
            quote: None,
            remove: true,
        },
    );
    carol.flush();
    // a banned voter's opinions never count
    alice.mod_action(assert_mod(
        &w.forum,
        &carol.did(),
        ModActionValue::Access {
            level: AccessLevel::Banned,
        },
    ));
    // nor a restricted voter's votes
    alice.mod_action(assert_mod(
        &w.forum,
        &bob.did(),
        ModActionValue::Restrict {
            capabilities: vec![Capability::Vote],
            nullify_votes: false,
        },
    ));
    alice.flush();
    w.resync(&[bob, carol]);
    let (code, msg) = carol.opinion_err(&w.bob_post, up());
    assert_eq!(code, ErrorCode::Blocked);
    assert!(msg.contains("banned"), "{msg}");
    let (code, msg) = bob.opinion_err(&w.welcome, up());
    assert_eq!(code, ErrorCode::Blocked);
    assert_eq!(msg, "you are restricted from voting in this forum");
}

#[test]
fn a_vote_by_a_voter_not_yet_established_is_recorded_with_a_notice() {
    let w = World::with_mode(MembershipMode::Open);
    // open mode, no grants: dave is a participant but neither a seed nor established
    let t = w.dave.opinion(&w.welcome, up());
    assert_eq!(t.notices.len(), 1, "{t:?}");
    assert_eq!(t.notices[0].code, NoticeCode::NotEstablished);
    assert_eq!(
        t.notices[0].message,
        "recorded; will count once you're established"
    );
    w.dave.flush();
    assert_eq!(
        w.dave
            .record(nsid::OPINION, &w.opinion_rkey(&w.welcome))
            .unwrap()["karma"],
        "up"
    );
}

// ---------------------------------------------------------------------------------------------
// Moderation

#[test]
fn hide_keep_and_remove_go_through_the_protocols() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    let c = carol.comment(&w.bob_post, None, "spam spam");
    w.resync(&[alice, bob]);
    // a non-authority cannot moderate in forum scope (the post's author is told about author
    // scope)
    let e = bob.mod_err(assert_mod(&w.forum, &c, hide()));
    assert_eq!(e.code, ErrorCode::Blocked);
    assert!(e.message.contains("author scope"), "{}", e.message);
    // the owner hides it, with a reason
    let mut intent = assert_mod(&w.forum, &c, hide());
    if let ModActionIntent::Assert { reason, .. } = &mut intent {
        *reason = Some("off topic".into());
    }
    let t = alice.mod_action(intent);
    alice.flush();
    let uri = alice.done(&t.ticket());
    let rkey = uri.rsplit('/').next().unwrap();
    let rec = alice.record(nsid::MOD_ACTION, rkey).unwrap();
    assert_eq!(rec["subject"], c.as_str());
    assert_eq!(rec["reason"], "off topic");
    assert!(
        rec["subjectCid"].is_string(),
        "negative actions carry the CID they saw"
    );
    assert_eq!(rec["action"]["$type"], nsid::MOD_ACTION_HIDE);
    assert_eq!(
        rec["action"].get("hidden"),
        None,
        "the default is not written"
    );
    // re-asserting the same key continues as an update (keep), with a revision of the old one
    let keep = alice.mod_action(assert_mod(
        &w.forum,
        &c,
        ModActionValue::Hide {
            hidden: false,
            spam: false,
            show_reason: true,
        },
    ));
    alice.flush();
    assert_eq!(alice.done(&keep.ticket()), uri);
    let rec = alice.record(nsid::MOD_ACTION, rkey).unwrap();
    assert_eq!(rec["action"]["hidden"], false);
    assert_eq!(
        rec["subjectCid"],
        carol
            .acct
            .cid(nsid::COMMENT, c.rsplit('/').next().unwrap())
            .unwrap()
            .to_string()
    );
    let revisions = alice.acct.list(nsid::REVISION);
    assert_eq!(revisions.len(), 1, "{revisions:?}");
    // removal: the removal protocol, with a delete revision
    let rm = ModActionIntent::Remove {
        forum: w.forum.clone(),
        scope: ModScope::Forum,
        subject: c.clone(),
        action_key: "hide".into(),
    };
    alice.mod_action(rm.clone());
    alice.flush();
    assert_eq!(alice.record(nsid::MOD_ACTION, rkey), None);
    assert_eq!(alice.acct.list(nsid::REVISION).len(), 2);
    // nothing left to remove
    w.resync(&[alice]);
    let e = alice.mod_err(rm);
    assert_eq!(e.code, ErrorCode::NotFound);
}

#[test]
fn rank_role_and_subject_rules() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    // bob becomes a moderator, carol an admin
    for (d, role) in [
        (bob.did(), ModRole::Moderator),
        (carol.did(), ModRole::Admin),
    ] {
        block_on(alice.engine.mods_set(
            &w.forum,
            &ModsChange::Add {
                did: d,
                role,
                title: None,
            },
        ))
        .unwrap();
        alice.flush();
    }
    let decl = alice.record(nsid::FORUM, "meta").unwrap();
    assert_eq!(decl["moderators"].as_array().unwrap().len(), 2);
    w.sync_all();
    let c = carol.comment(&w.welcome, None, "an admin's comment");
    w.resync(&[bob, carol]);
    // a moderator cannot hide an admin's comment
    let e = bob.mod_err(assert_mod(&w.forum, &c, hide()));
    assert_eq!(e.code, ErrorCode::Blocked);
    assert_eq!(
        e.message,
        "a moderator cannot moderate an admin (the higher rank decides; ask the owner or an admin)"
    );
    // nor curate
    let e = bob.mod_err(assert_mod(
        &w.forum,
        &w.welcome,
        ModActionValue::Tier {
            tier: Tier::Curated,
            notice: None,
        },
    ));
    assert_eq!(e.code, ErrorCode::Blocked);
    // subjects of the wrong kind
    let e = bob.mod_err(assert_mod(
        &w.forum,
        &w.welcome,
        ModActionValue::Access {
            level: AccessLevel::Banned,
        },
    ));
    assert_eq!(e.code, ErrorCode::InvalidInput);
    let e = bob.mod_err(assert_mod(
        &w.forum,
        &w.forum,
        ModActionValue::Lock { locked: true },
    ));
    assert_eq!(
        e.code,
        ErrorCode::InvalidInput,
        "forum-wide locks are reserved"
    );
    // an admin pins a post (positive: bound to the cached CID)
    let t = carol.mod_action(assert_mod(
        &w.forum,
        &w.bob_post,
        ModActionValue::Pin {
            scope: PinScope::Forum,
            pinned: true,
        },
    ));
    carol.flush();
    let uri = carol.done(&t.ticket());
    let rec = carol
        .record(nsid::MOD_ACTION, uri.rsplit('/').next().unwrap())
        .unwrap();
    assert_eq!(
        rec["subjectCid"],
        bob.acct
            .cid(nsid::POST, w.bob_post.rsplit('/').next().unwrap())
            .unwrap()
            .to_string()
    );
    // a restriction carries the retroactivity notice
    let t = carol.mod_action(assert_mod(
        &w.forum,
        &w.dave.did(),
        ModActionValue::Restrict {
            capabilities: vec![Capability::Comment, Capability::Post],
            nullify_votes: true,
        },
    ));
    assert_eq!(t.notices[0].code, NoticeCode::Retroactive);
    carol.flush();
    let uri = carol.done(&t.ticket());
    let rec = carol
        .record(nsid::MOD_ACTION, uri.rsplit('/').next().unwrap())
        .unwrap();
    assert_eq!(rec["action"]["capabilities"], json!(["post", "comment"]));
    assert_eq!(rec["action"]["nullifyVotes"], true);
}

#[test]
fn a_lock_keeps_the_visible_comments_and_is_refused_above_5000() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    // a small thread: the allow-list is its visible comments, sorted
    let c1 = carol.comment(&w.welcome, None, "first");
    w.resync(&[bob]);
    let c2 = bob.comment(&w.welcome, Some(&c1), "second");
    w.resync(&[alice]);
    let t = alice.mod_action(assert_mod(
        &w.forum,
        &w.welcome,
        ModActionValue::Lock { locked: true },
    ));
    alice.flush();
    let uri = alice.done(&t.ticket());
    let rec = alice
        .record(nsid::MOD_ACTION, uri.rsplit('/').next().unwrap())
        .unwrap();
    let mut want = vec![c1.clone(), c2.clone()];
    want.sort();
    assert_eq!(rec["action"]["allowed"], json!(want));

    // bob's post gets 5,001 visible comments (written straight into bob's repo)
    let tmpl_uri = bob.comment(&w.bob_post, None, "one");
    let tmpl_rkey = tmpl_uri.rsplit('/').next().unwrap().to_owned();
    let tmpl = bob.acct.get(nsid::COMMENT, &tmpl_rkey).unwrap();
    let prefix = tmpl_rkey.rsplit_once('-').unwrap().0.to_owned();
    let base = bob.host.now() - 10 * MINUTE;
    bob.acct.batch(|r| {
        for i in 0..5_000 {
            let k = format!("{prefix}-{}", tid_at_ms(base + i));
            r.put(nsid::COMMENT, &k, &tmpl);
        }
    });
    w.resync(&[alice]);
    let e = alice.mod_err(assert_mod(
        &w.forum,
        &w.bob_post,
        ModActionValue::Lock { locked: true },
    ));
    assert_eq!(e.code, ErrorCode::InvalidInput, "{}", e.message);
    assert!(
        e.message.contains("hide the thread instead"),
        "{}",
        e.message
    );
    assert_eq!(e.detail.as_ref().unwrap()["hint"], "useHide");
    assert_eq!(e.detail.as_ref().unwrap()["allowed"], 5_001);
    // hiding is the way
    alice.mod_action(assert_mod(&w.forum, &w.bob_post, hide()));
    alice.flush();
}

#[test]
fn author_scope_actions_follow_the_author_table() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    let c = carol.comment(&w.bob_post, None, "a comment on bob's post");
    w.resync(&[alice, bob]);
    let author = |subject: &str, action: ModActionValue| ModActionIntent::Assert {
        forum: w.forum.clone(),
        scope: ModScope::Author,
        subject: subject.into(),
        action,
        reason: None,
        until_ms: None,
    };
    // bob has no karma: the author gate (authorModPersonal, 10 in the preset) refuses
    let e = bob.mod_err(author(&c, hide()));
    assert_eq!(e.code, ErrorCode::Blocked, "{}", e.message);
    assert_eq!(e.detail.as_ref().unwrap()["gate"], "authorModPersonal");
    // actions author scope does not allow
    let e = bob.mod_err(author(
        &w.bob_post,
        ModActionValue::Tier {
            tier: Tier::Frontpage,
            notice: None,
        },
    ));
    assert_eq!(e.code, ErrorCode::InvalidInput);
    // the owner passes every gate: a lock of her own post in author scope
    let t = alice.mod_action(author(&w.welcome, ModActionValue::Lock { locked: true }));
    alice.flush();
    let uri = alice.done(&t.ticket());
    let rec = alice
        .record(nsid::MOD_ACTION, uri.rsplit('/').next().unwrap())
        .unwrap();
    assert_eq!(rec["scope"], "author");
    // not someone else's post
    let e = alice.mod_err(author(&w.bob_post, ModActionValue::Lock { locked: true }));
    assert_eq!(e.code, ErrorCode::Blocked);
}

#[test]
fn mods_rm_adopts_first() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    block_on(alice.engine.mods_set(
        &w.forum,
        &ModsChange::Add {
            did: bob.did(),
            role: ModRole::Moderator,
            title: Some("helper".into()),
        },
    ))
    .unwrap();
    alice.flush();
    let d = carol.comment(&w.welcome, None, "to be hidden");
    w.resync(&[bob]);
    let t = bob.mod_action(assert_mod(&w.forum, &d, hide()));
    bob.flush();
    let bobs = bob.done(&t.ticket());
    w.resync(&[alice]);

    // removing bob: mod adopt runs first, then the declaration edit
    let r = block_on(alice.engine.mods_set(
        &w.forum,
        &ModsChange::Remove {
            did: bob.did(),
            skip_adopt: false,
        },
    ))
    .unwrap();
    assert_eq!(r.notices.len(), 1);
    assert_eq!(r.notices[0].code, NoticeCode::AdoptedFirst);
    let writes = block_on(alice.engine.writes()).unwrap();
    let pending: Vec<&WriteTicket> = writes
        .iter()
        .filter(|t| matches!(t.state, WriteState::Pending { .. }))
        .collect();
    assert_eq!(pending.len(), 2, "{writes:?}");
    assert_eq!(pending[1].id, r.id, "the edit comes after the adoption");
    alice.flush();
    // alice's copy adopts bob's assertion (same key), and bob is no longer a moderator
    let rkey = bobs.rsplit('/').next().unwrap();
    let copy = alice.record(nsid::MOD_ACTION, rkey).unwrap();
    assert_eq!(copy["adopts"]["uri"], bobs.as_str());
    assert_eq!(copy["subject"], d.as_str());
    let decl = alice.record(nsid::FORUM, "meta").unwrap();
    assert_eq!(decl.get("moderators"), None, "{decl}");
}

#[test]
fn mods_rm_does_not_remove_when_adoption_fails() {
    let w = World::new();
    let (alice, bob) = (&w.alice, &w.bob);
    block_on(alice.engine.mods_set(
        &w.forum,
        &ModsChange::Add {
            did: bob.did(),
            role: ModRole::Moderator,
            title: None,
        },
    ))
    .unwrap();
    alice.flush();
    w.resync(&[alice, bob]);
    // bob's assertions cannot be listed: the adoption fails
    w.net.inject(
        Fault::error(400, "InvalidRequest", "no")
            .on_nsid("com.atproto.repo.listRecords")
            .on_repo(&bob.did()),
    );
    let r = block_on(alice.engine.mods_set(
        &w.forum,
        &ModsChange::Remove {
            did: bob.did(),
            skip_adopt: false,
        },
    ))
    .unwrap();
    let report = block_on(alice.engine.outbox_flush()).unwrap();
    // the adoption failed; the edit that depends on it failed unsent (a conflict)
    assert_eq!(
        (report.failed, report.conflicts, report.done),
        (1, 1, 0),
        "{report:?}"
    );
    match alice.state(&r.id) {
        WriteState::Failed { error } => {
            assert_eq!(error.code, ErrorCode::Conflict);
            assert!(
                error.message.contains("did not complete"),
                "{}",
                error.message
            );
        }
        other => panic!("{other:?}"),
    }
    let decl = alice.record(nsid::FORUM, "meta").unwrap();
    assert_eq!(
        decl["moderators"][0]["did"],
        bob.did().as_str(),
        "bob is still listed"
    );
    // only the owner edits the declaration
    let e = block_on(bob.engine.mods_set(
        &w.forum,
        &ModsChange::Remove {
            did: bob.did(),
            skip_adopt: true,
        },
    ))
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Blocked);
}

#[test]
fn forum_update_edits_the_declaration_and_keeps_the_moderators() {
    let w = World::new();
    let alice = &w.alice;
    block_on(alice.engine.mods_set(
        &w.forum,
        &ModsChange::Add {
            did: w.bob.did(),
            role: ModRole::Admin,
            title: None,
        },
    ))
    .unwrap();
    alice.flush();
    let mut d = ForumDraft::new("meta", "Meta, renamed");
    d.membership = MembershipMode::Open;
    d.description = Some("open now".into());
    block_on(alice.engine.forum_update(&w.forum, &d)).unwrap();
    alice.flush();
    let decl = alice.record(nsid::FORUM, "meta").unwrap();
    assert_eq!(decl["name"], "Meta, renamed");
    assert_eq!(decl["membership"], "open");
    assert_eq!(decl["moderators"][0]["role"], "admin");
    assert!(decl["updatedAt"].is_string());
    // the slug is fixed; moderators change with mods_set
    d.slug = "other".into();
    assert_eq!(
        block_on(alice.engine.forum_update(&w.forum, &d))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
}

#[test]
fn approve_and_reject_applicants() {
    let w = World::new();
    let (alice, dave) = (&w.alice, &w.dave);
    alice.sync(&w.forum);
    // dave is an applicant (a membership, no grant); reject dismisses him on this device
    let t = block_on(alice.engine.reject(&w.forum, &[dave.did()])).unwrap();
    assert!(t.is_empty(), "nothing public to remove");
    let r = block_on(alice.engine.rejected_applicants(&w.forum)).unwrap();
    assert!(r.contains(&dave.did()));
    // approval grants and forgets the rejection
    let t = block_on(alice.engine.approve(&w.forum, &[dave.did()])).unwrap();
    assert_eq!(t.len(), 1);
    alice.flush();
    assert!(
        block_on(alice.engine.rejected_applicants(&w.forum))
            .unwrap()
            .is_empty()
    );
    // rejecting a granted member removes the own grant
    alice.sync(&w.forum);
    let t = block_on(alice.engine.reject(&w.forum, &[dave.did()])).unwrap();
    assert_eq!(t.len(), 1);
    alice.flush();
    // only authorities approve
    let e = block_on(w.bob.engine.approve(&w.forum, &[dave.did()])).unwrap_err();
    assert_eq!(e.code, ErrorCode::Blocked);
}

// ---------------------------------------------------------------------------------------------
// Notifications and read state

fn notes(u: &User, all: bool) -> lessdugin_core::dto::NotificationList {
    block_on(
        u.engine
            .notifications(&NotificationsQuery { all, limit: 50 }),
    )
    .unwrap()
}

#[test]
fn notifications_come_from_first_seen_times() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    // the first computation records the baseline: nothing that was already synced is news
    assert!(notes(alice, true).items.is_empty());
    // bob replies to alice's post; alice syncs an hour later
    let reply = bob.comment(&w.welcome, None, "Thanks @alice.example.com!");
    let created = bob
        .acct
        .get(nsid::COMMENT, reply.rsplit('/').next().unwrap())
        .unwrap()["createdAt"]
        .as_str()
        .unwrap()
        .to_owned();
    w.tick(60 * MINUTE);
    let seen = alice.host.now();
    alice.sync(&w.forum);
    let n = notes(alice, false);
    assert_eq!(n.unread, 1);
    assert_eq!(n.items.len(), 1, "{n:?}");
    let item = &n.items[0];
    assert_eq!(item.reason, NotificationReason::Reply);
    assert_eq!(item.subject, reply);
    assert_eq!(item.actor.as_ref().unwrap().did, bob.did());
    assert_eq!(item.forum.as_deref(), Some(w.forum.as_str()));
    // first seen at alice's sync, not at its createdAt
    assert!(
        item.at_ms >= seen,
        "{} < {seen} (createdAt {created})",
        item.at_ms
    );
    assert!(!item.read);

    // a backdated comment still notifies: its first-seen time is new
    let tmpl = bob
        .acct
        .get(nsid::COMMENT, reply.rsplit('/').next().unwrap())
        .unwrap();
    let mut old = tmpl.clone();
    old["createdAt"] = json!("2001-01-01T00:00:00.000Z");
    let prefix = reply
        .rsplit('/')
        .next()
        .unwrap()
        .rsplit_once('-')
        .unwrap()
        .0
        .to_owned();
    let k = format!("{prefix}-{}", tid_at_ms(bob.host.now() - 30 * MINUTE));
    bob.acct.put(nsid::COMMENT, &k, &old);
    w.tick(MINUTE);
    alice.sync(&w.forum);
    let n = notes(alice, false);
    assert_eq!(n.unread, 2, "{n:?}");

    // reading the thread reads its notifications
    alice.take_events();
    block_on(alice.engine.mark_read(&w.welcome)).unwrap();
    let n = notes(alice, false);
    assert_eq!(n.unread, 0);
    assert_eq!(notes(alice, true).items.len(), 2);
    let ev = alice.take_events();
    assert!(
        ev.iter().any(
            |e| matches!(e, Event::Changed { topics } if topics.contains(&Topic::Notifications))
        ),
        "{ev:?}"
    );
    assert!(
        block_on(alice.engine.read_mark(&w.welcome))
            .unwrap()
            .is_some()
    );

    // a quote of carol's comment by bob
    let cc = carol.comment(&w.bob_post, None, "The quick brown fox.");
    assert!(notes(carol, true).items.is_empty());
    w.resync(&[bob]);
    let mut d = CommentDraft::new(&w.bob_post, Some(cc.clone()), "indeed");
    d.quote = Some(TextQuote {
        exact: "brown fox".into(),
        prefix: None,
        suffix: None,
        start: None,
    });
    let t = block_on(bob.engine.comment(&d)).unwrap();
    bob.flush();
    let q = bob.done(&t);
    let qrec = bob
        .acct
        .get(nsid::COMMENT, q.rsplit('/').next().unwrap())
        .unwrap();
    assert_eq!(qrec["quote"]["prefix"], "The quick ");
    w.resync(&[carol]);
    let n = notes(carol, false);
    // a reply to her comment is a Reply first
    assert_eq!(n.items[0].reason, NotificationReason::Reply);
}

#[test]
fn reacts_moderation_and_subscriptions_notify() {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    let bc = bob.comment(&w.welcome, None, "a comment");
    w.sync_all();
    for u in [alice, bob, carol] {
        assert!(notes(u, true).items.is_empty());
    }
    // carol subscribes to the forum: new posts notify her
    block_on(carol.engine.set_subscription(
        &Subscription::Forum {
            uri: w.forum.clone(),
        },
        true,
    ))
    .unwrap();
    let subs = block_on(carol.engine.subscriptions()).unwrap();
    assert_eq!(subs.len(), 1);
    // alice reacts on bob's post and hides bob's comment
    alice.opinion(&w.bob_post, react("insightful", Stance::For));
    alice.mod_action(assert_mod(&w.forum, &bc, hide()));
    alice.flush();
    let p = alice.post(&w.forum, "News", "Something new.");
    w.resync(&[bob, carol]);
    let n = notes(bob, false);
    let reasons: Vec<NotificationReason> = n.items.iter().map(|i| i.reason).collect();
    assert!(reasons.contains(&NotificationReason::Reacted), "{n:?}");
    assert!(reasons.contains(&NotificationReason::ModAction), "{n:?}");
    let m = n
        .items
        .iter()
        .find(|i| i.reason == NotificationReason::ModAction)
        .unwrap();
    assert_eq!(m.subject, bc);
    assert_eq!(m.preview.as_deref(), Some("hidden by a moderator"));
    assert_eq!(m.actor.as_ref().unwrap().did, alice.did());
    let r = n
        .items
        .iter()
        .find(|i| i.reason == NotificationReason::Reacted)
        .unwrap();
    assert_eq!(r.subject, w.bob_post);
    assert_eq!(r.preview.as_deref(), Some("insightful +1"));
    let n = notes(carol, false);
    assert_eq!(n.items.len(), 1, "{n:?}");
    assert_eq!(n.items[0].reason, NotificationReason::NewPost);
    assert_eq!(n.items[0].subject, p);
    // the same state again: nothing new
    let again = notes(bob, false);
    assert_eq!(again.items.len(), 2, "{again:?}");
    // mark one read, then all
    block_on(
        bob.engine
            .mark_notifications_read(std::slice::from_ref(&r.id)),
    )
    .unwrap();
    assert_eq!(notes(bob, false).unread, 1);
    block_on(bob.engine.mark_all_notifications_read()).unwrap();
    assert_eq!(notes(bob, false).unread, 0);
}

#[test]
fn unread_counts_per_forum_and_thread() {
    let w = World::new();
    let (alice, bob) = (&w.alice, &w.bob);
    let u = block_on(alice.engine.unread_counts()).unwrap();
    assert_eq!(u.forums.len(), 1);
    assert_eq!((u.forums[0].posts, u.forums[0].comments), (0, 0), "{u:?}");
    // alice views her thread; bob comments twice and posts
    block_on(alice.engine.mark_read(&w.welcome)).unwrap();
    bob.comment(&w.welcome, None, "one");
    bob.comment(&w.welcome, None, "two");
    bob.post(&w.forum, "Another", "post");
    w.tick(MINUTE);
    alice.sync(&w.forum);
    let u = block_on(alice.engine.unread_counts()).unwrap();
    let f = &u.forums[0];
    assert_eq!(f.forum, w.forum);
    assert_eq!((f.posts, f.comments), (1, 2), "{u:?}");
    assert_eq!(f.notifications, 2, "two replies to alice's post");
    assert_eq!(u.notifications, 2);
    // viewing the thread and the forum list resets them
    block_on(alice.engine.mark_read(&w.welcome)).unwrap();
    block_on(alice.engine.mark_read(&w.forum)).unwrap();
    let u = block_on(alice.engine.unread_counts()).unwrap();
    assert_eq!(
        (u.forums[0].posts, u.forums[0].comments, u.notifications),
        (0, 0, 0)
    );
}

#[test]
fn karma_changes_follow_the_cadence() {
    let w = World::new();
    let (bob, carol) = (&w.bob, &w.carol);
    block_on(bob.engine.set_karma_notifications(KarmaCadence::Realtime)).unwrap();
    assert_eq!(
        block_on(bob.engine.karma_notifications()).unwrap(),
        KarmaCadence::Realtime
    );
    assert!(notes(bob, true).items.is_empty());
    carol.opinion(&w.bob_post, up());
    carol.flush();
    w.tick(MINUTE);
    bob.sync(&w.forum);
    let n = notes(bob, false);
    let k: Vec<_> = n
        .items
        .iter()
        .filter(|i| i.reason == NotificationReason::KarmaChange)
        .collect();
    assert_eq!(k.len(), 1, "{n:?}");
    assert!(k[0].preview.as_deref().unwrap().starts_with('+'), "{k:?}");
}

// ---------------------------------------------------------------------------------------------
// Local lists

#[test]
fn local_lists_and_public_action_acknowledgements() {
    let w = World::new();
    let carol = &w.carol;
    // interstitials: once per kind and account
    assert!(
        block_on(carol.engine.acknowledged_public_actions())
            .unwrap()
            .is_empty()
    );
    assert_eq!(up().public_kind(), PublicActionKind::Vote);
    assert_eq!(
        react("x", Stance::For).public_kind(),
        PublicActionKind::React
    );
    block_on(
        carol
            .engine
            .acknowledge_public_action(PublicActionKind::Vote),
    )
    .unwrap();
    block_on(
        carol
            .engine
            .acknowledge_public_action(PublicActionKind::ModAction),
    )
    .unwrap();
    assert_eq!(
        block_on(carol.engine.acknowledged_public_actions()).unwrap(),
        [PublicActionKind::Vote, PublicActionKind::ModAction]
    );
    assert!(
        !block_on(
            carol
                .engine
                .public_action_acknowledged(PublicActionKind::React)
        )
        .unwrap()
    );
    // bookmarks, with the post's summary
    block_on(carol.engine.bookmark(&w.bob_post, true)).unwrap();
    block_on(carol.engine.bookmark(&w.welcome, true)).unwrap();
    let b = block_on(carol.engine.bookmarks()).unwrap();
    assert_eq!(b.len(), 2);
    assert!(b.iter().all(|e| e.post.is_some()));
    block_on(carol.engine.bookmark(&w.welcome, false)).unwrap();
    assert_eq!(block_on(carol.engine.bookmarks()).unwrap().len(), 1);
    // mutes
    block_on(carol.engine.mute(&w.dave.did(), true)).unwrap();
    let m = block_on(carol.engine.mutes()).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].did, w.dave.did());
    block_on(carol.engine.mute(&w.dave.did(), false)).unwrap();
    assert!(block_on(carol.engine.muted()).unwrap().is_empty());
    // subscriptions on and off
    let s = Subscription::User { did: w.bob.did() };
    block_on(carol.engine.set_subscription(&s, true)).unwrap();
    block_on(carol.engine.set_subscription(&s, false)).unwrap();
    assert!(block_on(carol.engine.subscriptions()).unwrap().is_empty());
}
