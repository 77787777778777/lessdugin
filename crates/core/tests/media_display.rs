// SPDX-License-Identifier: AGPL-3.0-or-later
//! The display half of the media pipeline over the fake network (`sync.md` §11): a reader's
//! engine fetches another author's media from THAT author's PDS (`load_media`), signed in or
//! not, and sniffs what it got; spilled bodies (`textBlob`) are fetched, decoded and rendered
//! (`lfm-1.md` §1.3); the views carry the autoload verdict (§11.3).
//!
//! The records are written straight into the authors' repos, as in `engine_views.rs`.

mod common;

use common::block_on;
use lessdugin_core::dto::{MediaView, PostView, SyncOpts};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_core::host::{Host, KvOp, KvStore};
use lessdugin_core::media::MediaOwner;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_testkit::fake_pds::{AccountHandle, Fault, tiny_png};
use lessdugin_testkit::repo::tid_at_ms;
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::{Value, json};

const MINUTE: i64 = 60_000;

struct Net {
    net: FakeNet,
    hosts: Vec<MemHost>,
}

impl Net {
    fn new() -> Self {
        let h = MemHost::new();
        let net = FakeNet::on(&h);
        let _ = net.default_pds();
        Self {
            net,
            hosts: vec![h],
        }
    }

    fn now(&self) -> i64 {
        self.hosts[0].now()
    }

    fn tick(&self, ms: i64) {
        for h in &self.hosts {
            h.advance_ms(ms);
        }
    }

    /// A new device signed in as `who`, or anonymous.
    fn device(&mut self, name: &str, who: Option<&AccountHandle>) -> Engine<MemHost> {
        let h = MemHost::new();
        h.set_now_ms(self.now());
        h.set_seed(self.hosts.len() as u64 + 1);
        self.net.install_as(&h, name);
        self.hosts.push(h.clone());
        let e = Engine::new(h, EngineConfig::default());
        if let Some(a) = who {
            block_on(e.add_account(AccountInfo::app_password(a.did().as_str(), a.pds_base())))
                .unwrap();
        }
        e
    }
}

fn dt(ms: i64) -> String {
    lessdugin_atp::syntax::Datetime::from_millis(ms)
        .unwrap()
        .as_str()
        .to_owned()
}

/// `owner`'s open forum, with its membership.
fn forum(n: &Net, owner: &AccountHandle) -> String {
    owner.put(
        nsid::FORUM,
        "meta",
        &json!({
            "$type": nsid::FORUM, "name": "Pics", "membership": "open",
            "defaultTier": "frontpage", "createdAt": dt(n.now()),
        }),
    );
    let f = owner.uri(nsid::FORUM, "meta");
    owner.put(
        nsid::MEMBERSHIP,
        &keys::membership_rkey(&f).unwrap(),
        &json!({"$type": nsid::MEMBERSHIP, "forum": f, "createdAt": dt(n.now())}),
    );
    n.tick(MINUTE);
    f
}

/// A post by `who` in `f` whose body is `body` (an `NS.markdown` value).
fn post(n: &Net, who: &AccountHandle, f: &str, body: Value) -> String {
    let at = n.now();
    let v = json!({
        "$type": nsid::POST, "forum": f, "title": "Pictures", "body": body,
        "createdAt": dt(at),
    });
    let uri = format!(
        "at://{}/{}/{}",
        who.did(),
        nsid::POST,
        keys::post_rkey(f, tid_at_ms(at).as_str()).unwrap()
    );
    let check = lessdugin_lexicon::validity::check_uri(
        &lessdugin_atp::syntax::AtUri::parse(&uri).unwrap(),
        &v,
    );
    assert!(check.valid, "{check:?}");
    let rkey = uri.rsplit('/').next().unwrap().to_owned();
    who.put(nsid::POST, &rkey, &v);
    n.tick(MINUTE);
    uri
}

fn image_body(blob: &Value) -> Value {
    json!({
        "$type": nsid::MARKDOWN, "profile": "lfm/1",
        "text": format!("Look: ![a dot](cid:{})", blob["ref"]["$link"].as_str().unwrap()),
        "media": [{"blob": blob, "alt": "a dot"}],
    })
}

fn read(e: &Engine<MemHost>, f: &str, uri: &str) -> PostView {
    let r = block_on(e.sync_forum(f, &SyncOpts::default())).unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    block_on(e.get_post(uri)).unwrap()
}

/// The `getBlob` requests the network saw, as (PDS base, `did` parameter).
fn blob_requests(n: &Net) -> Vec<(String, String)> {
    n.net
        .requests_for("com.atproto.sync.getBlob")
        .into_iter()
        .map(|r| (r.host, r.params.get("did").cloned().unwrap_or_default()))
        .collect()
}

#[test]
fn media_comes_from_its_authors_pds_for_any_reader() {
    // m4-1: load_media asked the ACTIVE account's PDS for the reader's own DID, so another
    // author's media never loaded (BlobNotFound) and a signed-out reader got auth_required
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let carol = n
        .net
        .add_pds("carol.example.com")
        .add_account("carol.example.com");
    let f = forum(&n, &bob);
    let blob = bob.upload_blob(tiny_png(7), "image/png");
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    let uri = post(&n, &bob, &f, image_body(&blob));

    for (name, who) in [("anon", None), ("carol", Some(&carol))] {
        let e = n.device(name, who);
        let pv = read(&e, &f, &uri);
        let view: &MediaView = &pv.body.media[0];
        assert_eq!(view.cid, cid);
        assert_eq!(view.author, bob.did().as_str());
        n.net.clear_requests();
        let loaded = block_on(e.load_media(&cid, Some(MediaOwner::of_view(view)))).unwrap();
        assert_eq!(loaded.cid, cid, "{name}");
        assert_eq!(loaded.size, tiny_png(7).len() as u64);
        assert_eq!(loaded.mime.as_deref(), Some("image/png"), "{name}");
        assert_eq!(
            blob_requests(&n),
            [(bob.pds_base(), bob.did().to_string())],
            "{name}: from bob's PDS, as bob"
        );
        // the verified bytes are the host's to hand out (an object URL, a file path)
        assert_eq!(
            block_on(e.host().blob_read(&cid, 0, 4096)).unwrap(),
            tiny_png(7)
        );
        // only the author's DID works too: the sniffed type decides alone
        let again = block_on(e.load_media(&cid, Some(MediaOwner::author(bob.did().as_str()))));
        assert_eq!(again.unwrap().mime.as_deref(), Some("image/png"));
    }
}

#[test]
fn the_view_is_not_trusted_for_the_pds_and_its_declaration_must_agree() {
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let blob = bob.upload_blob(tiny_png(8), "image/png");
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    let uri = post(&n, &bob, &f, image_body(&blob));
    let e = n.device("anon", None);
    let mut view = read(&e, &f, &uri).body.media[0].clone();

    // a view naming another PDS is not followed: the DID document decides
    view.pds = "https://elsewhere.example.com".into();
    n.net.clear_requests();
    block_on(e.load_media(&cid, Some(MediaOwner::of_view(&view)))).unwrap();
    assert_eq!(blob_requests(&n)[0].0, bob.pds_base());

    // a declared type the bytes do not have: a download card, never inline
    view.mime = "video/mp4".into();
    let card = block_on(e.load_media(&cid, Some(MediaOwner::of_view(&view)))).unwrap();
    assert_eq!(card.mime, None);

    // a view of another blob is refused
    let err = block_on(e.load_media("bafkreiother", Some(MediaOwner::of_view(&view)))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // the declared size is the cap: a body longer than it is aborted, and nothing is kept
    block_on(e.host().blob_delete(&cid)).unwrap();
    view.mime = "image/png".into();
    view.size_bytes = 10;
    let err = block_on(e.load_media(&cid, Some(MediaOwner::of_view(&view)))).unwrap_err();
    assert_eq!(err.code, ErrorCode::TooLarge);
    assert_eq!(block_on(e.host().blob_open(&cid)).unwrap(), None);
}

#[test]
fn a_blob_the_pds_does_not_have_is_missing_and_purged() {
    // sync.md §11.4: "Blob not found" is not retried, and a cached copy is deleted
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let e = n.device("anon", None);
    let gone = lessdugin_atp::cid::for_raw(b"never uploaded").to_string();
    let err =
        block_on(e.load_media(&gone, Some(MediaOwner::author(bob.did().as_str())))).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.detail.unwrap()["kind"], "mediaMissing");
}

// --- spilled bodies (m4-5) ---------------------------------------------------------------------

/// A spilled body of `source` bytes, uploaded to `who`'s PDS.
fn spilled_body(who: &AccountHandle, source: &[u8]) -> Value {
    let blob = who.upload_blob(source.to_vec(), "text/markdown");
    json!({"$type": nsid::MARKDOWN, "profile": "lfm/1", "textBlob": blob})
}

fn spilled_post(n: &Net, who: &AccountHandle, f: &str, source: &[u8]) -> String {
    let at = n.now();
    let v = json!({
        "$type": nsid::POST, "forum": f, "title": "Long", "body": spilled_body(who, source),
        "excerpt": "An excerpt", "createdAt": dt(at),
    });
    let rkey = keys::post_rkey(f, tid_at_ms(at).as_str()).unwrap();
    who.put(nsid::POST, &rkey, &v);
    n.tick(MINUTE);
    who.uri(nsid::POST, &rkey)
}

#[test]
fn a_spilled_body_is_fetched_from_its_authors_pds_and_rendered() {
    // m4-5: a body above 64 KiB MUST spill (data-model.md §5.3), and every client showed only
    // its excerpt and "[the body is stored as a blob and is not loaded yet]"
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let mut source = "\u{FEFF}# Chapter one\n\nIt was a dark night.\n\n".to_owned();
    while source.len() <= 65_536 {
        source.push_str("More words in the long body. ");
    }
    let uri = spilled_post(&n, &bob, &f, source.as_bytes());
    let e = n.device("anon", None);
    n.net.clear_requests();
    let pv = read(&e, &f, &uri);
    assert_eq!(
        pv.body.notices,
        Vec::<String>::new(),
        "{:?}",
        pv.body.notices
    );
    assert!(
        pv.body.html.contains("Chapter one"),
        "{}",
        &pv.body.html[..200]
    );
    assert!(pv.body.text.starts_with("Chapter one It was a dark night."));
    assert_eq!(pv.body.toc.len(), 1);
    assert_eq!(blob_requests(&n), [(bob.pds_base(), bob.did().to_string())]);
    // rendered once: the next view is the cache, with no request
    n.net.clear_requests();
    let again = block_on(e.get_thread(&uri, &Default::default())).unwrap();
    let lessdugin_core::dto::ThreadRoot::Post(root) = again.post else {
        panic!("the thread has its post");
    };
    assert_eq!(root.body.html, pv.body.html);
    assert!(blob_requests(&n).is_empty());
}

#[test]
fn a_hidden_spilled_post_is_never_fetched() {
    // sync.md §11.4: hidden content is never prefetched. get_post fetched the textBlob of any
    // post it was asked for, and kept it, while the view rendered the hidden post empty
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let uri = spilled_post(&n, &bob, &f, b"# Spam\n\nBuy now.");
    // bob, the forum's owner, hides it (forum scope)
    let action = json!({"$type": format!("{}#hide", nsid::MOD_ACTION), "hidden": true});
    let rkey = keys::mod_action_rkey(
        lessdugin_lexicon::ns::NS,
        &f,
        None,
        &uri,
        action.as_object().unwrap(),
    )
    .unwrap();
    bob.put(
        nsid::MOD_ACTION,
        &rkey,
        &json!({
            "$type": nsid::MOD_ACTION, "forum": f, "subject": uri, "action": action,
            "createdAt": dt(n.now()),
        }),
    );
    n.tick(MINUTE);
    let e = n.device("anon", None);
    let r = block_on(e.sync_forum(&f, &SyncOpts::default())).unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    n.net.clear_requests();
    // the view lists nothing of it, and nothing was asked of bob's PDS
    let pv = block_on(e.get_post(&uri)).unwrap();
    assert_eq!((pv.body.text.as_str(), pv.body.html.as_str()), ("", ""));
    let _ = block_on(e.get_thread(&uri, &Default::default()));
    assert!(blob_requests(&n).is_empty(), "{:?}", blob_requests(&n));
    let blob = lessdugin_atp::cid::for_raw(b"# Spam\n\nBuy now.").to_string();
    assert_eq!(block_on(e.host().blob_open(&blob)).unwrap(), None);
}

#[test]
fn a_spilled_body_that_is_not_utf8_is_unreadable() {
    // data-model.md §5.2 rule 3: invalid UTF-8 renders the "unreadable body" placeholder
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let uri = spilled_post(&n, &bob, &f, b"caf\xe9 au lait");
    let e = n.device("anon", None);
    let pv = read(&e, &f, &uri);
    assert_eq!(pv.body.notices, ["[unreadable body: not valid UTF-8]"]);
    assert_eq!(pv.body.text, "");
}

#[test]
fn a_spilled_body_the_pds_does_not_have_is_missing_and_not_asked_again() {
    // sync.md §11.1: HTTP 400 "Blob not found" is missing, not retried; the body was pending and
    // asked for again every BODY_BLOB_RETRY_MS
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    // a record naming a blob bob's PDS never stored
    let at = n.now();
    let missing = json!({
        "$type": "blob", "ref": {"$link": lessdugin_atp::cid::for_raw(b"gone").to_string()},
        "mimeType": "text/markdown", "size": 4,
    });
    let v = json!({
        "$type": nsid::POST, "forum": f, "title": "Long",
        "body": {"$type": nsid::MARKDOWN, "textBlob": missing},
        "excerpt": "An excerpt", "createdAt": dt(at),
    });
    let rkey = keys::post_rkey(&f, tid_at_ms(at).as_str()).unwrap();
    bob.put(nsid::POST, &rkey, &v);
    let uri = bob.uri(nsid::POST, &rkey);
    let e = n.device("anon", None);
    n.net.clear_requests();
    let pv = read(&e, &f, &uri);
    assert_eq!(
        pv.body.notices,
        [lessdugin_core::model::render::NOTICE_MISSING_BODY]
    );
    assert_eq!(pv.body.text, "");
    assert_eq!(blob_requests(&n).len(), 1);
    // not asked again, even once BODY_BLOB_RETRY_MS has passed
    n.tick(lessdugin_core::limits::BODY_BLOB_RETRY_MS);
    n.net.clear_requests();
    let again = block_on(e.get_post(&uri)).unwrap();
    assert_eq!(again.body.notices, pv.body.notices);
    assert!(blob_requests(&n).is_empty());
}

#[test]
fn a_spilled_body_that_cannot_be_fetched_stays_pending_and_is_asked_again_later() {
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let uri = spilled_post(&n, &bob, &f, b"# The whole body");
    // the first getBlob drops the connection
    n.net
        .inject(Fault::disconnect().on_nsid("com.atproto.sync.getBlob").once());
    let e = n.device("anon", None);
    let pv = read(&e, &f, &uri);
    assert_eq!(
        pv.body.notices,
        [lessdugin_core::model::render::NOTICE_SPILLED]
    );
    assert_eq!(pv.body.text, "An excerpt");
    // not asked again on every view
    n.net.clear_requests();
    block_on(e.get_post(&uri)).unwrap();
    assert!(blob_requests(&n).is_empty());
    // but once BODY_BLOB_RETRY_MS has passed
    n.tick(lessdugin_core::limits::BODY_BLOB_RETRY_MS);
    let pv = block_on(e.get_post(&uri)).unwrap();
    assert_eq!(blob_requests(&n).len(), 1);
    assert_eq!(pv.body.text, "The whole body");
}

#[test]
fn a_slow_authors_pds_does_not_hold_back_the_thread() {
    // get_thread and get_post waited for the textBlob before they returned anything: a PDS
    // that trickles bytes held back the post and everyone's comments (natively for up to
    // BLOB_TOTAL_TIMEOUT_MS, in the browser without a limit)
    use futures::FutureExt as _;
    use lessdugin_core::limits::{BODY_BLOB_RETRY_MS, BODY_BLOB_WAIT_MS};
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let source = b"# The whole body";
    let uri = spilled_post(&n, &bob, &f, source);
    let e = n.device("anon", None);
    let r = block_on(e.sync_forum(&f, &SyncOpts::default())).unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    // bob's PDS stalls: the view comes back once BODY_BLOB_WAIT_MS is up (MemHost's sleep moves
    // the clock, so one poll does it), with the body pending
    e.host().hold_blob_fetches(true);
    e.host().clear_requests();
    let t = e
        .get_thread(&uri, &Default::default())
        .now_or_never()
        .expect("a view that waits only BODY_BLOB_WAIT_MS")
        .unwrap()
        .post;
    let lessdugin_core::dto::ThreadRoot::Post(root) = t else {
        panic!("the thread has its post");
    };
    assert_eq!(root.body.notices, [lessdugin_core::model::render::NOTICE_SPILLED]);
    assert_eq!(root.body.text, "An excerpt");
    assert_eq!(e.host().sleeps(), [BODY_BLOB_WAIT_MS]);
    // and the next view does not wait again: polled once, it is ready
    let pv = e.get_post(&uri).now_or_never().expect("a view that does not wait");
    assert_eq!(pv.unwrap().body.text, "An excerpt");
    // a copy the host stored meanwhile (the browser's download ran on) is taken, unasked
    let blob = lessdugin_atp::cid::for_raw(source).to_string();
    e.host().put_blob(&blob, source.to_vec());
    n.net.clear_requests();
    let pv = block_on(e.get_post(&uri)).unwrap();
    assert_eq!(pv.body.text, "The whole body");
    assert!(blob_requests(&n).is_empty());

    // natively the download stops with the wait: it is asked for again later
    let uri = spilled_post(&n, &bob, &f, b"# Another body");
    let r = block_on(e.sync_forum(&f, &SyncOpts::default())).unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    assert_eq!(block_on(e.get_post(&uri)).unwrap().body.text, "An excerpt");
    e.host().hold_blob_fetches(false);
    n.net.clear_requests();
    assert_eq!(block_on(e.get_post(&uri)).unwrap().body.text, "An excerpt");
    assert!(blob_requests(&n).is_empty());
    n.tick(BODY_BLOB_RETRY_MS);
    assert_eq!(block_on(e.get_post(&uri)).unwrap().body.text, "Another body");
    assert_eq!(blob_requests(&n).len(), 1);
}

// --- the autoload verdict (m4-4) ---------------------------------------------------------------

#[test]
fn views_carry_the_autoload_verdict() {
    // m4-4: MediaView.autoload was always false; the policy had no caller
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("pds1.host.bsky.network")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let png = bob.upload_blob(tiny_png(9), "image/png");
    let shown = post(&n, &bob, &f, image_body(&png));
    let video = bob.upload_blob(b"\0\0\0\x18ftypisom\0\0\0\0isommp42".to_vec(), "video/mp4");
    let clip = post(&n, &bob, &f, image_body(&video));
    let e = n.device("anon", None);
    // the owner (in S) on a Bluesky host, a visible post, a small image: it loads by itself
    assert!(read(&e, &f, &shown).body.media[0].autoload);
    // a video never does
    assert!(!read(&e, &f, &clip).body.media[0].autoload);
    // nor anything once the device's switch is off
    block_on(e.host().state().write(vec![KvOp::Put(
        lessdugin_core::key!("loc", "setting", "mediaAutoload"),
        b"off".to_vec(),
    )]))
    .unwrap();
    assert!(!block_on(e.get_post(&shown)).unwrap().body.media[0].autoload);
}

#[test]
fn a_labelled_post_or_an_unknown_host_asks_first() {
    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("bob.example.com")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let png = bob.upload_blob(tiny_png(10), "image/png");
    // bob's own PDS serves one participant: not a host the forum already talks to
    let plain = post(&n, &bob, &f, image_body(&png));
    let e = n.device("anon", None);
    assert!(!read(&e, &f, &plain).body.media[0].autoload);
    // with five participants on it, it is (AUTOLOAD_HOST_MIN_PARTICIPANTS, by identity rows):
    // four moderators on bob's PDS, so they are participants (A)
    let pds = n.net.pds(&bob.pds_base()).unwrap();
    let mods: Vec<Value> = (0..4)
        .map(|i| {
            let m = pds.add_account(&format!("m{i}.example.com"));
            json!({"did": m.did().as_str(), "role": "moderator"})
        })
        .collect();
    bob.put(
        nsid::FORUM,
        "meta",
        &json!({
            "$type": nsid::FORUM, "name": "Pics", "membership": "open",
            "defaultTier": "frontpage", "createdAt": dt(n.now()), "moderators": mods,
        }),
    );
    n.tick(MINUTE);
    let e = n.device("anon2", None);
    assert!(read(&e, &f, &plain).body.media[0].autoload);

    let mut n = Net::new();
    let bob = n
        .net
        .add_pds("pds2.host.bsky.network")
        .add_account("bob.example.com");
    let f = forum(&n, &bob);
    let png = bob.upload_blob(tiny_png(11), "image/png");
    let at = n.now();
    let v = json!({
        "$type": nsid::POST, "forum": f, "title": "Pictures", "body": image_body(&png),
        "labels": {"$type": "com.atproto.label.defs#selfLabels", "values": [{"val": "graphic-media"}]},
        "createdAt": dt(at),
    });
    let rkey = keys::post_rkey(&f, tid_at_ms(at).as_str()).unwrap();
    bob.put(nsid::POST, &rkey, &v);
    let labelled = bob.uri(nsid::POST, &rkey);
    let e = n.device("anon", None);
    let pv = read(&e, &f, &labelled);
    assert_eq!(pv.post.labels, ["graphic-media"]);
    assert!(!pv.body.media[0].autoload);
}
