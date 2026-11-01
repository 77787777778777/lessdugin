// SPDX-License-Identifier: AGPL-3.0-or-later
//! Focused M5 sequence-write, local-search and cached-user integration tests.

mod common;

use common::block_on;
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_core::dto::{
    BodyDraft, ForumDraft, ForumRole, ItemState, ModActionIntent, ModActionValue, ModScope,
    PostDraft, SearchItem, SearchQuery, SequenceChapterDraft, SequenceDraft, SequencePosition,
    SyncOpts, WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_core::host::Host;
use lessdugin_core::store::Observation;
use lessdugin_core::store::admit::{Expect, LexiconValidator, admit_value};
use lessdugin_core::store::records::RecordStore;
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::json;

const MINUTE: i64 = 60_000;

struct Device {
    host: MemHost,
    engine: Engine<MemHost>,
    account: AccountHandle,
}

impl Device {
    fn new(net: &FakeNet, host: MemHost, account: AccountHandle) -> Self {
        let engine = Engine::new(host.clone(), EngineConfig::default());
        block_on(engine.add_account(AccountInfo::app_password(
            account.did().as_str(),
            account.pds_base(),
        )))
        .unwrap();
        let _ = net;
        Self {
            host,
            engine,
            account,
        }
    }

    fn flush(&self) {
        let report = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!((report.failed, report.conflicts), (0, 0));
    }

    fn done(&self, ticket: &WriteTicket) -> String {
        match block_on(self.engine.write_status(&ticket.id))
            .unwrap()
            .unwrap()
            .state
        {
            WriteState::Done { uri, .. } => uri,
            state => panic!("write is {state:?}"),
        }
    }

    fn cache_record(
        &self,
        author: &Device,
        collection: &str,
        rkey: &str,
        value: &serde_json::Value,
    ) {
        let uri = format!("at://{}/{collection}/{rkey}", author.account.did());
        let at = AtUri::parse(&uri).unwrap();
        let did = Did::parse(at.did().as_str()).unwrap();
        let expect = Expect {
            did: &did,
            collection,
            rkey: Some(rkey),
        };
        let cid = author.account.cid(collection, rkey).unwrap().to_string();
        let admitted = admit_value(
            Some(&uri),
            Some(&cid),
            Some(value),
            serde_json::to_vec(value).unwrap().len(),
            &expect,
            &LexiconValidator,
        )
        .unwrap();
        let store = RecordStore::new(self.host.cache());
        let mut batch = store.batch();
        block_on(
            batch.put_version(admitted.into_version(Observation::point_fetch(
                "",
                self.host.now_ms(),
                self.host.now_ms(),
            ))),
        )
        .unwrap();
        block_on(batch.commit()).unwrap();
    }

    fn sync(&self, forum: &str) {
        let report = block_on(self.engine.sync_forum(forum, &SyncOpts::default())).unwrap();
        assert!(!report.partial, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
    }

    fn post(&self, forum: &str, title: &str, body: &str) -> String {
        let ticket =
            block_on(self.engine.publish(&PostDraft::article(forum, title, body))).unwrap();
        self.flush();
        self.done(&ticket)
    }
}

struct World {
    alice: Device,
    bob: Device,
    forum: String,
    first: String,
    second: String,
    comment: String,
}

impl World {
    fn new() -> Self {
        let host = MemHost::new();
        let net = FakeNet::on(&host);
        let _ = net.default_pds();
        let bob_host = MemHost::new();
        bob_host.set_seed(2);
        net.install_as(&bob_host, "bob");
        let alice = Device::new(&net, host, net.add_account("alice.example.com"));
        let bob = Device::new(&net, bob_host, net.add_account("bob.example.com"));
        let mut forum = ForumDraft::new("meta", "Meta");
        forum.membership = lessdugin_core::dto::MembershipMode::Open;
        let ticket = block_on(alice.engine.forum_create(&forum)).unwrap();
        alice.flush();
        let forum = alice.done(&ticket);
        block_on(bob.engine.forum_open(&forum)).unwrap();
        block_on(bob.engine.join(&forum, None)).unwrap();
        bob.flush();
        let first = alice.post(&forum, "Café notes", "αβ Café and shared text.");
        let second = bob.post(&forum, "Bob's post", "Shared text from Bob.");
        alice.host.advance_ms(MINUTE);
        bob.host.advance_ms(MINUTE);
        alice.sync(&forum);
        bob.sync(&forum);
        let ticket = block_on(bob.engine.comment(&lessdugin_core::dto::CommentDraft::new(
            &first,
            None,
            "Bob's recent comment",
        )))
        .unwrap();
        bob.flush();
        let comment = bob.done(&ticket);
        let world = Self {
            alice,
            bob,
            forum,
            first,
            second,
            comment,
        };
        world.tick();
        world
    }

    fn tick(&self) {
        self.alice.host.advance_ms(MINUTE);
        self.bob.host.advance_ms(MINUTE);
        self.alice.sync(&self.forum);
        self.bob.sync(&self.forum);
    }
}

fn sequence_draft(world: &World) -> SequenceDraft {
    SequenceDraft {
        forum: world.forum.clone(),
        title: "Reading list".into(),
        body: Some(BodyDraft::text("Introduction")),
        chapters: vec![
            SequenceChapterDraft {
                title: Some("One".into()),
                body: Some(BodyDraft::text("Chapter body")),
                posts: vec![world.first.clone()],
            },
            SequenceChapterDraft {
                title: Some("Two".into()),
                body: None,
                posts: vec![],
            },
        ],
    }
}

#[test]
fn sequence_writes_preserve_record_fields_and_validate_targets() {
    let world = World::new();
    let created = block_on(world.alice.engine.seq_create(&sequence_draft(&world))).unwrap();
    world.alice.flush();
    let uri = world.alice.done(&created);
    let rkey = uri.rsplit('/').next().unwrap();
    let original = world.alice.account.get(nsid::SEQUENCE, rkey).unwrap();
    assert_eq!(original["chapters"][0]["body"]["text"], "Chapter body");
    let created_at = original["createdAt"].clone();

    // Unknown top-level and chapter fields survive a full draft edit, as does createdAt.
    let mut unknown = original.clone();
    unknown["futureField"] = json!({"kept": true});
    unknown["chapters"][0]["futureChapterField"] = json!("kept");
    world.alice.account.put(nsid::SEQUENCE, rkey, &unknown);
    world.tick();
    let mut edited = sequence_draft(&world);
    edited.title = "Edited list".into();
    edited.chapters[0].title = Some("Edited chapter".into());
    block_on(world.alice.engine.seq_edit(&uri, &edited)).unwrap();
    world.alice.flush();
    let record = world.alice.account.get(nsid::SEQUENCE, rkey).unwrap();
    assert_eq!(record["title"], "Edited list");
    assert_eq!(record["createdAt"], created_at);
    assert_eq!(record["futureField"], json!({"kept": true}));
    assert_eq!(record["chapters"][0]["futureChapterField"], "kept");

    block_on(world.alice.engine.seq_add(
        &uri,
        &world.second,
        &SequencePosition {
            chapter: 0,
            index: 1,
        },
    ))
    .unwrap();
    world.alice.flush();
    block_on(world.alice.engine.seq_move(
        &uri,
        &SequencePosition {
            chapter: 0,
            index: 0,
        },
        &SequencePosition {
            chapter: 1,
            index: 0,
        },
    ))
    .unwrap();
    world.alice.flush();
    let record = world.alice.account.get(nsid::SEQUENCE, rkey).unwrap();
    assert_eq!(record["chapters"][0]["posts"], json!([world.second]));
    assert_eq!(record["chapters"][1]["posts"], json!([world.first]));
    block_on(world.alice.engine.seq_remove(&uri, &world.second)).unwrap();
    world.alice.flush();
    let record = world.alice.account.get(nsid::SEQUENCE, rkey).unwrap();
    assert_eq!(record["chapters"][0]["posts"], json!([]));

    let other = block_on(
        world
            .alice
            .engine
            .forum_create(&ForumDraft::new("other", "Other")),
    )
    .unwrap();
    world.alice.flush();
    let other = world.alice.done(&other);
    let mut wrong_forum = sequence_draft(&world);
    wrong_forum.forum = other;
    assert_eq!(
        block_on(world.alice.engine.seq_edit(&uri, &wrong_forum))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        block_on(world.alice.engine.seq_add(
            &world.first,
            &world.second,
            &SequencePosition {
                chapter: 0,
                index: 0,
            },
        ))
        .unwrap_err()
        .code,
        ErrorCode::InvalidRef
    );
    assert_eq!(
        block_on(world.bob.engine.seq_remove(&uri, &world.first))
            .unwrap_err()
            .code,
        ErrorCode::AuthRequired
    );
}

#[test]
fn search_is_unicode_safe_filtered_and_never_exposes_hidden_content() {
    let world = World::new();
    let hidden = world
        .bob
        .post(&world.forum, "Secretneedle title", "Secretneedle body");
    world.tick();
    let hide = ModActionIntent::Assert {
        forum: world.forum.clone(),
        scope: ModScope::Forum,
        subject: hidden.clone(),
        action: ModActionValue::Hide {
            hidden: true,
            spam: false,
            show_reason: true,
        },
        reason: None,
        until_ms: None,
    };
    block_on(world.alice.engine.mod_action(&hide)).unwrap();
    world.alice.flush();
    world.tick();

    let query = |q: &str, author: Option<String>| SearchQuery {
        q: q.into(),
        forum: Some(world.forum.clone()),
        author,
        limit: 1000,
    };
    let hits = block_on(world.bob.engine.search(&query("CAFÉ", None))).unwrap();
    assert_eq!(hits.hits.len(), 1);
    let SearchItem::Post(post) = &hits.hits[0].item else {
        panic!("expected a post")
    };
    assert_eq!(post.uri, world.first);
    assert_eq!(hits.hits[0].highlights, vec![(0, 4), (14, 18)]);

    let hits = block_on(
        world
            .bob
            .engine
            .search(&query("shared", Some(world.bob.account.did().to_string()))),
    )
    .unwrap();
    assert_eq!(hits.hits.len(), 1);
    let SearchItem::Post(post) = &hits.hits[0].item else {
        panic!("expected Bob's post")
    };
    assert_eq!(post.uri, world.second);
    assert!(
        block_on(world.bob.engine.search(&query("Secretneedle", None)))
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(
        block_on(world.bob.engine.search(&query("article", None)))
            .unwrap()
            .hits
            .is_empty(),
        "record kind and tags are not searchable body text"
    );

    // The mute lens collapses rather than reveals or removes searchable content.
    block_on(
        world
            .bob
            .engine
            .mute(world.alice.account.did().as_str(), true),
    )
    .unwrap();
    let hits = block_on(world.bob.engine.search(&query("Café", None))).unwrap();
    let SearchItem::Post(post) = &hits.hits[0].item else {
        panic!("expected a post")
    };
    assert!(matches!(post.state, ItemState::Collapsed { .. }));
}

#[test]
fn user_uses_cached_profile_and_listed_recent_content() {
    let world = World::new();
    let profile = json!({
        "$type": nsid::PROFILE,
        "displayName": "Alice",
        "bio": {"$type": nsid::MARKDOWN, "profile": "lfm/1", "text": "About **Alice**"},
        "links": [{"uri": "https://example.com/alice", "label": "Home"}],
        "pinned": [world.first],
    });
    world.alice.account.put(nsid::PROFILE, "self", &profile);
    world
        .bob
        .cache_record(&world.alice, nsid::PROFILE, "self", &profile);
    let view = block_on(
        world
            .bob
            .engine
            .user(Some(&world.forum), world.alice.account.did().as_str()),
    )
    .unwrap();
    assert_eq!(view.actor.display_name.as_deref(), Some("Alice"));
    assert_eq!(view.status, ForumRole::Owner);
    assert!(view.karma.is_some());
    assert_eq!(view.bio.unwrap().text, "About Alice");
    assert_eq!(view.links[0].host, "example.com");
    assert_eq!(view.pinned[0].uri, world.first);
    assert!(view.recent_posts.iter().any(|post| post.uri == world.first));

    let global = block_on(
        world
            .bob
            .engine
            .user(None, world.bob.account.did().as_str()),
    )
    .unwrap();
    assert_eq!(global.status, ForumRole::None);
    assert!(global.karma.is_none());
    assert!(global.bio.is_none());
    assert!(global.links.is_empty());
    assert_eq!(global.recent_comments[0].uri, world.comment);
}
