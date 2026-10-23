// SPDX-License-Identifier: AGPL-3.0-or-later
//! M3 views end to end over the fake network (`MemHost` + `FakePds`): the records are written
//! straight into the authors' repos (as any client of the protocol could), the readers' engines
//! sync them and answer `list_posts`, `get_thread`, `recent_discussion`, `members`, `mod_queue`,
//! `mod_log` and `raw` from the full fold (`social.md` §1).
//!
//! - every displayed state: `Visible`, `Collapsed(NewUser | LowScore)`,
//!   `Hidden(Removed | Banned | Restricted | AuthorModerated | Locked)` and their tombstones;
//! - every post sort and filter, every comment sort;
//! - scores, the viewer's own opinion, reacts, badges, viewer capabilities;
//! - fail closed: a moderator's PDS going down never brings hidden spam back;
//! - determinism: two engines over the same records give byte-identical JSON.

mod common;

use common::block_on;
use lessdugin_core::dto::{
    Banner, CommentSort, ForumRole, HiddenReason, ItemState, ListPostsQuery, MembersQuery,
    ModLogStatus, PostFilter, PostSort, PostView, SyncOpts, ThreadOpts, ThreadView,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_testkit::fake_pds::{AccountHandle, Fault};
use lessdugin_testkit::repo::tid_at_ms;
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::{Map, Value, json};

const MINUTE: i64 = 60_000;

// ---------------------------------------------------------------------------------------------
// The network, its authors and the readers' devices

struct Net {
    net: FakeNet,
    /// Every device's host; the first one runs the network's clock.
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

    /// Moves every clock.
    fn tick(&self, ms: i64) {
        for h in &self.hosts {
            h.advance_ms(ms);
        }
    }

    /// A new device, with its own host (and seed), signed in as `who` (or anonymous).
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

fn md(text: &str) -> Value {
    json!({"$type": nsid::MARKDOWN, "profile": "lfm/1", "text": text})
}

/// A forum whose records are written straight into the repos, each a minute after the last.
struct Forum<'n> {
    n: &'n Net,
    uri: String,
}

/// A record written: its uri and cid.
#[derive(Clone, Debug)]
struct Rec {
    uri: String,
    cid: String,
}

impl Rec {
    fn strong(&self) -> Value {
        json!({"uri": self.uri, "cid": self.cid})
    }
}

impl<'n> Forum<'n> {
    fn create(n: &'n Net, owner: &AccountHandle, mode: &str, extra: Value) -> Self {
        let mut decl = json!({
            "$type": nsid::FORUM, "name": "Meta", "membership": mode,
            "defaultTier": "frontpage", "createdAt": dt(n.now()),
        });
        for (k, v) in extra.as_object().unwrap() {
            decl[k] = v.clone();
        }
        owner.put(nsid::FORUM, "meta", &decl);
        let f = Self {
            n,
            uri: owner.uri(nsid::FORUM, "meta"),
        };
        f.join(owner);
        f
    }

    fn put(&self, who: &AccountHandle, coll: &str, rkey: &str, v: &Value) -> Rec {
        let check = lessdugin_lexicon::validity::check_uri(
            &lessdugin_atp::syntax::AtUri::parse(&who.uri(coll, rkey)).unwrap(),
            v,
        );
        assert!(check.valid, "{coll}/{rkey}: {check:?}");
        let cid = who.put(coll, rkey, v).to_string();
        self.n.tick(MINUTE);
        Rec {
            uri: who.uri(coll, rkey),
            cid,
        }
    }

    fn join(&self, who: &AccountHandle) {
        let v =
            json!({"$type": nsid::MEMBERSHIP, "forum": self.uri, "createdAt": dt(self.n.now())});
        self.put(
            who,
            nsid::MEMBERSHIP,
            &keys::membership_rkey(&self.uri).unwrap(),
            &v,
        );
    }

    fn post(&self, who: &AccountHandle, title: &str, text: &str, extra: Value) -> Rec {
        let at = self.n.now();
        let mut v = json!({
            "$type": nsid::POST, "forum": self.uri, "title": title, "body": md(text),
            "createdAt": dt(at),
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        let rkey = keys::post_rkey(&self.uri, tid_at_ms(at).as_str()).unwrap();
        self.put(who, nsid::POST, &rkey, &v)
    }

    fn comment(&self, who: &AccountHandle, root: &Rec, parent: Option<&Rec>, text: &str) -> Rec {
        let at = self.n.now();
        let mut v = json!({
            "$type": nsid::COMMENT, "forum": self.uri, "root": root.strong(),
            "body": md(text), "createdAt": dt(at),
        });
        if let Some(p) = parent {
            v["parent"] = p.strong();
        }
        let rkey = keys::comment_rkey(NS, &self.uri, &root.uri, tid_at_ms(at).as_str()).unwrap();
        self.put(who, nsid::COMMENT, &rkey, &v)
    }

    /// An opinion of `who` on `subject` with the given axes.
    fn opinion(&self, who: &AccountHandle, subject: &Rec, axes: Value) -> Rec {
        let mut v = json!({
            "$type": nsid::OPINION, "forum": self.uri, "subject": subject.strong(),
            "createdAt": dt(self.n.now()),
        });
        for (k, x) in axes.as_object().unwrap() {
            v[k] = x.clone();
        }
        let rkey = keys::opinion_rkey(NS, &self.uri, &subject.uri).unwrap();
        self.put(who, nsid::OPINION, &rkey, &v)
    }

    /// A modAction of `who` (`scope`: `None` for forum scope).
    fn act(
        &self,
        who: &AccountHandle,
        scope: Option<&str>,
        subject: &str,
        cid: Option<&str>,
        action: Value,
        reason: Option<&str>,
    ) -> Rec {
        let name = action["$type"].as_str().unwrap().to_owned();
        let mut a: Map<String, Value> = action.as_object().unwrap().clone();
        a.insert(
            "$type".into(),
            Value::String(format!("{}#{name}", nsid::MOD_ACTION)),
        );
        let mut v = json!({
            "$type": nsid::MOD_ACTION, "forum": self.uri, "subject": subject,
            "action": a, "createdAt": dt(self.n.now()),
        });
        if let Some(s) = scope {
            v["scope"] = json!(s);
        }
        if let Some(c) = cid {
            v["subjectCid"] = json!(c);
        }
        if let Some(r) = reason {
            v["reason"] = json!(r);
        }
        let rkey = keys::mod_action_rkey(NS, &self.uri, scope, subject, &a).unwrap();
        self.put(who, nsid::MOD_ACTION, &rkey, &v)
    }
}

fn sync(e: &Engine<MemHost>, forum: &str) {
    let r = block_on(e.sync_forum(forum, &SyncOpts::default())).unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
}

fn list(e: &Engine<MemHost>, forum: &str, sort: PostSort, filter: PostFilter) -> Vec<String> {
    let mut q = ListPostsQuery::new(forum);
    q.sort = sort;
    q.filter = filter;
    block_on(e.list_posts(&q))
        .unwrap()
        .items
        .into_iter()
        .map(|p| p.uri)
        .collect()
}

fn thread(e: &Engine<MemHost>, root: &Rec, sort: CommentSort) -> ThreadView {
    let opts = ThreadOpts {
        sort,
        ..ThreadOpts::default()
    };
    block_on(e.get_thread(&root.uri, &opts)).unwrap()
}

fn post(e: &Engine<MemHost>, p: &Rec) -> PostView {
    block_on(e.get_post(&p.uri)).unwrap()
}

/// The state of a comment in a thread view (`None`: not displayed).
fn state_in<'t>(t: &'t ThreadView, c: &Rec) -> Option<&'t ItemState> {
    t.comments.iter().find(|n| n.uri == c.uri).map(|n| &n.state)
}

/// The "moderation data unavailable" banner (`social.md` §4.11) is among `b`.
fn unavailable(b: &[Banner]) -> bool {
    b.iter().any(|b| b.code == "moderationUnavailable")
}

fn hidden(reason: HiddenReason) -> ItemState {
    ItemState::Hidden { reason }
}

fn tomb(reason: HiddenReason) -> ItemState {
    ItemState::Tombstone { reason }
}

// ---------------------------------------------------------------------------------------------
// States, sorts, filters

/// The cast of an open forum: alice owns it, mo moderates, bob and carol are granted (seeds),
/// nina and sam joined without a grant (newcomers), bea is banned, rita is restricted from
/// commenting. Seeds reply to or vote on the newcomers' items, so the closure (`sync.md` §4.3)
/// brings their repos into the sync.
struct Cast {
    alice: AccountHandle,
    mo: AccountHandle,
    bob: AccountHandle,
    carol: AccountHandle,
    nina: AccountHandle,
    sam: AccountHandle,
    bea: AccountHandle,
    rita: AccountHandle,
    /// Never joined.
    dan: AccountHandle,
}

impl Cast {
    fn new(n: &Net) -> Self {
        let a = |h: &str| n.net.add_account(&format!("{h}.example.com"));
        Self {
            alice: a("alice"),
            mo: a("mo"),
            bob: a("bob"),
            carol: a("carol"),
            nina: a("nina"),
            sam: a("sam"),
            bea: a("bea"),
            rita: a("rita"),
            dan: a("dan"),
        }
    }
}

struct Scene<'n> {
    f: Forum<'n>,
    welcome: Rec,
    bob_post: Rec,
    carol_post: Rec,
    nina_post: Rec,
    spam: Rec,
    quick: Rec,
    /// Comments under `welcome`.
    c_bob: Rec,
    c_carol_reply: Rec,
    c_bea: Rec,
    c_bob_under_bea: Rec,
    c_nina: Rec,
    c_low: Rec,
    c_rita: Rec,
    /// By a non-member, with bob's reply under it.
    c_dan: Rec,
    c_bob_under_dan: Rec,
    /// Under `bob_post`: nina's comment that bob hides in author scope.
    c_author_hidden: Rec,
    /// Under `carol_post`, after alice locked it.
    c_locked: Rec,
    /// The last comment, under `c_carol_reply`.
    c_late: Rec,
    spam_hide: Rec,
}

fn scene<'n>(n: &'n Net, c: &Cast) -> Scene<'n> {
    let gates = json!({"reactCreate": 0, "reactSecond": 0, "reactAnti": 0,
                       "authorModFrontpage": 0, "authorModPersonal": 0});
    let f = Forum::create(
        n,
        &c.alice,
        "open",
        json!({
            "moderators": [{"did": c.mo.did().as_str(), "role": "moderator"}],
            "policy": {"gates": gates, "display": {"collapseBelow": -1, "newUsers": "collapse"}},
        }),
    );
    for u in [&c.mo, &c.bob, &c.carol, &c.nina, &c.sam, &c.bea, &c.rita] {
        f.join(u);
    }
    let access = |d: &AccountHandle, level: &str| {
        f.act(
            &c.alice,
            None,
            d.did().as_str(),
            None,
            json!({"$type": "access", "level": level}),
            None,
        );
    };
    access(&c.bob, "member");
    access(&c.carol, "member");
    access(&c.bea, "banned");
    f.act(
        &c.alice,
        None,
        c.rita.did().as_str(),
        None,
        json!({"$type": "restrict", "capabilities": ["comment"]}),
        None,
    );

    // posts, a minute apart
    let welcome = f.post(&c.alice, "Welcome", "Hello everyone, be kind.", json!({}));
    let bob_post = f.post(&c.bob, "Bob's", "Bob writes.", json!({}));
    let carol_post = f.post(&c.carol, "Carol's", "Carol writes.", json!({}));
    let nina_post = f.post(&c.nina, "Nina's", "New here.", json!({}));
    let spam = f.post(&c.sam, "BUY PILLS", "spam spam", json!({}));
    let quick = f.post(&c.bob, "", "A quick thought.", json!({"kind": "quickTake"}));

    // comments under the welcome post
    let c_bob = f.comment(&c.bob, &welcome, None, "Thanks!");
    let c_carol_reply = f.comment(&c.carol, &welcome, Some(&c_bob), "Indeed.");
    let c_bea = f.comment(&c.bea, &welcome, None, "I was banned.");
    let c_bob_under_bea = f.comment(&c.bob, &welcome, Some(&c_bea), "Bye bea.");
    let c_nina = f.comment(&c.nina, &welcome, None, "Hi all, I'm new.");
    let c_low = f.comment(&c.bob, &welcome, None, "A bad take.");
    let c_rita = f.comment(&c.rita, &welcome, None, "Restricted me.");
    let c_dan = f.comment(&c.dan, &welcome, None, "Not a member.");
    let c_bob_under_dan = f.comment(&c.bob, &welcome, Some(&c_dan), "Join first.");
    let c_author_hidden = f.comment(&c.nina, &bob_post, None, "Off topic on bob's.");

    // moderation: pins, tiers, a spam hide with a private reason, a lock, an author hide
    f.act(
        &c.alice,
        None,
        &welcome.uri,
        Some(&welcome.cid),
        json!({"$type": "pin", "scope": "forum", "pinned": true}),
        None,
    );
    f.act(
        &c.alice,
        None,
        &bob_post.uri,
        Some(&bob_post.cid),
        json!({"$type": "tier", "tier": "curated"}),
        None,
    );
    f.act(
        &c.mo,
        None,
        &carol_post.uri,
        None,
        json!({"$type": "tier", "tier": "personal"}),
        None,
    );
    let spam_hide = f.act(
        &c.mo,
        None,
        &spam.uri,
        None,
        json!({"$type": "hide", "hidden": true, "spam": true, "showReason": false}),
        Some("pill spam"),
    );
    f.act(
        &c.alice,
        None,
        &carol_post.uri,
        None,
        json!({"$type": "lock", "locked": true}),
        None,
    );
    f.act(
        &c.bob,
        Some("author"),
        &c_author_hidden.uri,
        None,
        json!({"$type": "hide", "hidden": true}),
        None,
    );
    let c_locked = f.comment(&c.bob, &carol_post, None, "After the lock.");

    // votes: bob's post is the top one; bob's bad take sinks (K[bob] stays above carol's); a seed's vote on the spam brings
    // sam into the closure; bob supports carol's quoted react on the welcome post
    f.opinion(&c.alice, &bob_post, json!({"karma": "up"}));
    f.opinion(
        &c.carol,
        &bob_post,
        json!({"karma": "strongUp", "agreement": "agree"}),
    );
    f.opinion(&c.alice, &c_low, json!({"karma": "down"}));
    f.opinion(&c.carol, &c_low, json!({"karma": "down"}));
    f.opinion(&c.alice, &quick, json!({"karma": "up"}));
    f.opinion(&c.bob, &spam, json!({"karma": "down"}));
    f.opinion(&c.bob, &nina_post, json!({"karma": "up"}));
    f.opinion(&c.carol, &c_rita, json!({"karma": "up"}));
    f.opinion(
        &c.carol,
        &welcome,
        json!({"karma": "up", "reacts": [{"react": "crux", "quote": {"exact": "be kind"}}]}),
    );
    f.opinion(
        &c.bob,
        &welcome,
        json!({"reacts": [{"react": "crux", "quote": {"exact": "be kind"}}]}),
    );
    // a newcomer's vote: recorded, not counted
    f.opinion(&c.nina, &welcome, json!({"karma": "up"}));
    // the last word: a reply deep under bob's first comment
    let c_late = f.comment(&c.alice, &welcome, Some(&c_carol_reply), "Late reply.");
    Scene {
        f,
        welcome,
        bob_post,
        carol_post,
        nina_post,
        spam,
        quick,
        c_bob,
        c_carol_reply,
        c_bea,
        c_bob_under_bea,
        c_nina,
        c_low,
        c_rita,
        c_dan,
        c_bob_under_dan,
        c_author_hidden,
        c_locked,
        c_late,
        spam_hide,
    }
}

#[test]
fn states_scores_and_badges() {
    let mut n = Net::new();
    let c = Cast::new(&n);
    let carol = n.device("carol", Some(&c.carol));
    let nina = n.device("nina", Some(&c.nina));
    let s = scene(&n, &c);
    let f = &s.f.uri;
    sync(&carol, f);
    sync(&nina, f);

    // a newcomer's vote is "not yet counted", for everyone; nina sees her own as such
    assert_eq!(post(&carol, &s.welcome).post.score.unweighted, 1);
    let mine = post(&nina, &s.welcome).post.score.mine.unwrap();
    assert!(mine.not_yet_counted && !mine.pending, "{mine:?}");
    let caps = block_on(nina.forum_open(f)).unwrap().viewer.unwrap();
    assert!(caps.newcomer && !caps.weighted && caps.vote, "{caps:?}");

    // the welcome thread: every comment state, tombstones keep the shape
    let t = thread(&carol, &s.welcome, CommentSort::Old);
    assert_eq!(state_in(&t, &s.c_bob), Some(&ItemState::Visible));
    assert_eq!(state_in(&t, &s.c_carol_reply), Some(&ItemState::Visible));
    assert_eq!(
        state_in(&t, &s.c_bea),
        Some(&tomb(HiddenReason::Banned)),
        "a banned comment with a listed reply is a tombstone"
    );
    assert_eq!(state_in(&t, &s.c_bob_under_bea), Some(&ItemState::Visible));
    assert_eq!(
        state_in(&t, &s.c_nina),
        Some(&ItemState::Collapsed {
            reason: lessdugin_core::dto::CollapseReason::NewUser
        })
    );
    assert_eq!(
        state_in(&t, &s.c_low),
        Some(&ItemState::Collapsed {
            reason: lessdugin_core::dto::CollapseReason::LowScore
        })
    );
    assert_eq!(
        state_in(&t, &s.c_rita),
        None,
        "a restricted comment without replies is not displayed"
    );
    // a non-member's comment is never synced (closure members need a confirmed membership,
    // `sync.md` §4.3): bob's reply hangs under a Missing placeholder (the model tests cover
    // its `Hidden(NotParticipant)` state when the record is cached)
    let dan = t.comments.iter().find(|x| x.uri == s.c_dan.uri).unwrap();
    assert!(dan.missing && dan.depth == 1, "{dan:?}");
    let under = t
        .comments
        .iter()
        .find(|x| x.uri == s.c_bob_under_dan.uri)
        .unwrap();
    assert_eq!((under.depth, &under.state), (2, &ItemState::Visible));
    let tb = t.comments.iter().find(|x| x.uri == s.c_bea.uri).unwrap();
    assert!(tb.body.html.is_empty(), "a tombstone shows no body");
    // the raw view shows every state, hidden ones included
    let raw = |r: &Rec| block_on(carol.raw(&r.uri)).unwrap();
    assert_eq!(raw(&s.c_rita).state, Some(hidden(HiddenReason::Restricted)));
    let t = thread(&carol, &s.bob_post, CommentSort::Magic);
    assert!(t.comments.is_empty(), "{t:?}");
    assert_eq!(
        raw(&s.c_author_hidden).state,
        Some(hidden(HiddenReason::AuthorModerated))
    );
    assert_eq!(
        raw(&s.c_locked).state,
        Some(hidden(HiddenReason::Locked)),
        "a comment after the lock"
    );

    // the spam is hidden as removed; its reason is the moderators' (showReason false)
    let spam = raw(&s.spam);
    match &spam.state {
        Some(ItemState::Hidden {
            reason: HiddenReason::Removed { spam, reason, .. },
        }) => {
            assert!(*spam);
            assert_eq!(
                reason, &None,
                "showReason false: no reason for a non-moderator"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        spam.assertions
            .iter()
            .any(|a| a["uri"] == s.spam_hide.uri.as_str() && a["status"] == "effective"),
        "{:?}",
        spam.assertions
    );
    let pv = post(&carol, &s.spam);
    assert!(pv.body.html.is_empty() && pv.post.title.is_none());

    // scores: the implicit self-vote, the weighted votes, the viewer's own opinion
    let bp = post(&carol, &s.bob_post);
    assert!(bp.post.score.score >= 3, "{:?}", bp.post.score);
    assert_eq!(bp.post.score.agreement, Some(1));
    let mine = bp.post.score.mine.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(mine).unwrap()["karma"],
        "strongUp",
        "{mine:?}"
    );
    assert_eq!(bp.post.tier, lessdugin_core::dto::Tier::Curated);

    // badges: a forum pin and a lock
    let wp = post(&carol, &s.welcome);
    assert!(wp.post.pinned);
    assert!(post(&carol, &s.carol_post).post.locked);
    // reacts: carol and bob both react crux on the same quote; the creator is the one with the
    // greater karma (bob), carol seconded it (both pass the 0 gates); it is anchored
    let crux = wp.reacts.iter().find(|r| r.react == "crux").unwrap();
    assert_eq!(crux.creator.as_deref(), Some(c.bob.did().as_str()));
    assert_eq!(crux.for_count, 2);
    assert_eq!(crux.quote.as_ref().unwrap().exact, "be kind");
    assert!(crux.anchor.is_some(), "{crux:?}");
    assert_eq!(
        serde_json::to_value(crux.mine).unwrap(),
        "for",
        "the viewer's stance"
    );

    // the forum view: role and capabilities of the viewer
    let fv = block_on(carol.forum_open(f)).unwrap();
    assert_eq!(fv.my_role, ForumRole::Member);
    let caps = fv.viewer.unwrap();
    assert!(caps.vote && caps.comment && caps.react_create && !caps.moderate);

    // the lens (§10.3): muting bob collapses his listed items for carol only
    block_on(carol.mute(c.bob.did().as_str(), true)).unwrap();
    let t = thread(&carol, &s.welcome, CommentSort::Old);
    assert_eq!(
        state_in(&t, &s.c_bob),
        Some(&ItemState::Collapsed {
            reason: lessdugin_core::dto::CollapseReason::Muted
        })
    );
    assert_eq!(
        state_in(&t, &s.c_bea),
        Some(&tomb(HiddenReason::Banned)),
        "the lens changes no hidden state"
    );
}

#[test]
fn post_sorts_and_filters() {
    let mut n = Net::new();
    let c = Cast::new(&n);
    let carol = n.device("carol", Some(&c.carol));
    let s = scene(&n, &c);
    let f = &s.f.uri;
    sync(&carol, f);
    let u = |r: &Rec| r.uri.clone();

    // frontpage (the default): tier ≥ frontpage, no quick takes, nothing hidden
    let front = list(&carol, f, PostSort::New, PostFilter::Frontpage);
    assert_eq!(front, [u(&s.nina_post), u(&s.bob_post), u(&s.welcome)]);
    // all: every tier and kind, still nothing hidden
    let all = list(&carol, f, PostSort::New, PostFilter::All);
    assert_eq!(
        all,
        [
            u(&s.quick),
            u(&s.nina_post),
            u(&s.carol_post),
            u(&s.bob_post),
            u(&s.welcome)
        ]
    );
    assert!(!all.contains(&s.spam.uri));
    let old = list(&carol, f, PostSort::Old, PostFilter::All);
    assert_eq!(old, all.iter().rev().cloned().collect::<Vec<_>>());
    // curated: the curated tier only
    assert_eq!(
        list(&carol, f, PostSort::New, PostFilter::Curated),
        [u(&s.bob_post)]
    );
    // magic: the forum pin first, then hot
    let magic = list(&carol, f, PostSort::Magic, PostFilter::All);
    assert_eq!(magic[0], s.welcome.uri, "{magic:?}");
    assert_eq!(magic.len(), all.len());
    // top: by score
    let top = list(&carol, f, PostSort::Top, PostFilter::All);
    assert_eq!(top[0], s.bob_post.uri, "{top:?}");
    // recentComments: the thread with the newest comment first (carol's post: its only
    // comment is hidden, so it has no activity)
    let rc = list(&carol, f, PostSort::RecentComments, PostFilter::All);
    assert_eq!(rc[0], s.welcome.uri, "{rc:?}");
    assert_eq!(rc.len(), all.len());
    // the default query is magic + frontpage
    let d = block_on(carol.list_posts(&ListPostsQuery::new(f))).unwrap();
    assert_eq!((d.sort, d.filter), (PostSort::Magic, PostFilter::Frontpage));

    // recent discussion: the welcome thread, with its newest visible comments first
    let rd = block_on(carol.recent_discussion(f, None, 0)).unwrap();
    assert_eq!(rd.items[0].post.uri, s.welcome.uri);
    let firsts: Vec<&str> = rd.items[0]
        .comments
        .iter()
        .map(|x| x.uri.as_str())
        .collect();
    assert_eq!(
        firsts.first().copied(),
        Some(s.c_late.uri.as_str()),
        "{firsts:?}"
    );
    // only Visible comments: the collapsed ones are not in it
    assert!(!firsts.contains(&s.c_low.uri.as_str()) && !firsts.contains(&s.c_nina.uri.as_str()));
}

#[test]
fn comment_sorts() {
    let mut n = Net::new();
    let c = Cast::new(&n);
    let carol = n.device("carol", Some(&c.carol));
    let s = scene(&n, &c);
    sync(&carol, &s.f.uri);
    let tops = |sort: CommentSort| -> Vec<String> {
        let t = thread(&carol, &s.welcome, sort);
        let firsts: Vec<_> = t.comments.into_iter().filter(|x| x.depth == 1).collect();
        // the Missing placeholder (dan's comment) sorts after every comment sibling
        assert_eq!(
            firsts.last().map(|x| (x.uri.as_str(), x.missing)),
            Some((s.c_dan.uri.as_str(), true)),
            "{sort:?}"
        );
        firsts
            .into_iter()
            .filter(|x| !x.missing)
            .map(|x| x.uri)
            .collect()
    };
    let old = tops(CommentSort::Old);
    assert_eq!(
        old,
        [
            s.c_bob.uri.clone(),
            s.c_bea.uri.clone(),
            s.c_nina.uri.clone(),
            s.c_low.uri.clone()
        ]
    );
    let new = tops(CommentSort::New);
    assert_eq!(new, old.iter().rev().cloned().collect::<Vec<_>>());
    // top: the downvoted comment sinks to the bottom
    let top = tops(CommentSort::Top);
    assert_eq!(top.last(), Some(&s.c_low.uri), "{top:?}");
    // latestReply: by the newest Visible comment of each subtree; alice's late reply lifts
    // bob's (oldest) comment to the top
    let lr = tops(CommentSort::LatestReply);
    assert_eq!(
        lr,
        [
            s.c_bob.uri.clone(),
            s.c_low.uri.clone(),
            s.c_nina.uri.clone(),
            s.c_bea.uri.clone()
        ]
    );
    let magic = tops(CommentSort::Magic);
    assert_eq!(magic.len(), old.len());
    // children follow their parent (pre-order) in every sort
    for sort in [
        CommentSort::Magic,
        CommentSort::Top,
        CommentSort::New,
        CommentSort::Old,
        CommentSort::LatestReply,
    ] {
        let t = thread(&carol, &s.welcome, sort);
        let i = |r: &Rec| t.comments.iter().position(|x| x.uri == r.uri).unwrap();
        assert_eq!(i(&s.c_carol_reply), i(&s.c_bob) + 1, "{sort:?}");
        assert_eq!(i(&s.c_bob_under_bea), i(&s.c_bea) + 1, "{sort:?}");
    }
}

#[test]
fn moderators_see_reasons_the_queue_and_the_log() {
    let mut n = Net::new();
    let c = Cast::new(&n);
    let mo = n.device("mo", Some(&c.mo));
    let carol = n.device("carol", Some(&c.carol));
    let s = scene(&n, &c);
    let f = &s.f.uri;
    sync(&mo, f);
    sync(&carol, f);

    // a hide with showReason false: the reason is shown to the authorities only
    match block_on(mo.raw(&s.spam.uri)).unwrap().state {
        Some(ItemState::Hidden {
            reason: HiddenReason::Removed { reason, .. },
        }) => assert_eq!(reason.as_deref(), Some("pill spam")),
        other => panic!("{other:?}"),
    }
    // the mod log is public; its private reasons are not
    let log = block_on(carol.mod_log(f, None, 0)).unwrap();
    let e = log
        .entries
        .iter()
        .find(|e| e.uri == s.spam_hide.uri)
        .unwrap();
    assert_eq!(e.status, ModLogStatus::Effective);
    assert_eq!(e.reason, None);
    let log = block_on(mo.mod_log(f, None, 0)).unwrap();
    let e = log
        .entries
        .iter()
        .find(|e| e.uri == s.spam_hide.uri)
        .unwrap();
    assert_eq!(e.reason.as_deref(), Some("pill spam"));
    assert!(
        log.entries.iter().any(|e| e.author_scope),
        "bob's author-scope hide is in the log"
    );
    // paging
    let p1 = block_on(mo.mod_log(f, None, 2)).unwrap();
    assert_eq!(p1.entries.len(), 2);
    let p2 = block_on(mo.mod_log(f, p1.cursor.as_deref(), 2)).unwrap();
    assert_eq!(p2.entries[0].uri, log.entries[2].uri);

    // the mod queue: moderators only; open mode lists the newcomers' newest items
    let q = block_on(mo.mod_queue(f)).unwrap();
    assert!(!q.items.is_empty(), "{q:?}");
    let e = block_on(carol.mod_queue(f)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Blocked);
    let fv = block_on(mo.forum_open(f)).unwrap();
    assert_eq!(fv.my_role, ForumRole::Moderator);
    assert!(fv.viewer.unwrap().moderate);

    // members: roles and flags
    let m = block_on(mo.members(f, &MembersQuery::default())).unwrap();
    let role = |a: &AccountHandle| {
        m.iter()
            .find(|x| x.actor.did == a.did().as_str())
            .map(|x| (x.status, x.banned, !x.restricted.is_empty()))
    };
    assert_eq!(role(&c.alice), Some((ForumRole::Owner, false, false)));
    assert_eq!(role(&c.mo), Some((ForumRole::Moderator, false, false)));
    assert_eq!(role(&c.bob), Some((ForumRole::Member, false, false)));
    assert_eq!(role(&c.nina), Some((ForumRole::Newcomer, false, false)));
    assert_eq!(role(&c.bea).map(|r| r.1), Some(true));
    assert_eq!(role(&c.rita).map(|r| r.2), Some(true));
}

// ---------------------------------------------------------------------------------------------
// Fail closed

/// A moderator hides spam; then the moderator's PDS goes down. A reader who had listed the
/// moderator's repo keeps the cached hide (`Stale` never degrades); a reader who never could
/// list it sees a degraded view, where the newcomer's spam is hidden as
/// `ModerationUnavailable`, under the "moderation unavailable" banner. The spam never reappears.
#[test]
fn a_moderator_going_offline_never_resurrects_hidden_spam() {
    let mut n = Net::new();
    let c = Cast::new(&n);
    let early = n.device("early", Some(&c.carol));
    let late = n.device("late", None);
    let s = scene(&n, &c);
    let f = &s.f.uri;
    sync(&early, f);
    let visible = |e: &Engine<MemHost>| list(e, f, PostSort::New, PostFilter::All);
    assert!(!visible(&early).contains(&s.spam.uri));

    // mo's PDS stops answering for mo's repo
    n.net
        .inject(Fault::status(503).on_repo(c.mo.did().as_str()));
    n.tick(2 * 24 * 60 * MINUTE);
    let _ = block_on(early.sync_forum(f, &SyncOpts::default()));
    assert!(
        !visible(&early).contains(&s.spam.uri),
        "the cached hide applies"
    );
    let pl = block_on(early.list_posts(&ListPostsQuery::new(f))).unwrap();
    assert!(
        !pl.completeness.degraded && !unavailable(&pl.banners),
        "a Stale authority does not degrade the view: {:?}",
        pl.completeness
    );
    assert_eq!(
        block_on(early.raw(&s.spam.uri))
            .unwrap()
            .state
            .map(|st| matches!(
                st,
                ItemState::Hidden {
                    reason: HiddenReason::Removed { .. }
                }
            )),
        Some(true)
    );

    // a reader who never listed mo's repo: degraded, and only S's content is shown
    let r = block_on(late.sync_forum(f, &SyncOpts::default())).unwrap();
    assert!(r.degraded || r.repos_unavailable > 0, "{r:?}");
    let posts = block_on(late.list_posts(&{
        let mut q = ListPostsQuery::new(f);
        q.sort = PostSort::New;
        q.filter = PostFilter::All;
        q
    }))
    .unwrap();
    assert!(posts.completeness.degraded);
    assert!(unavailable(&posts.banners), "{:?}", posts.banners);
    let uris: Vec<&str> = posts.items.iter().map(|p| p.uri.as_str()).collect();
    assert!(!uris.contains(&s.spam.uri.as_str()), "{uris:?}");
    assert!(
        !uris.contains(&s.nina_post.uri.as_str()),
        "newcomers' content waits for moderation"
    );
    assert!(
        uris.contains(&s.bob_post.uri.as_str()),
        "a seed's post stays"
    );
    if let Ok(raw) = block_on(late.raw(&s.spam.uri)) {
        assert!(
            matches!(raw.state, None | Some(ItemState::Hidden { .. })),
            "{:?}",
            raw.state
        );
    }
    let t = thread(&late, &s.welcome, CommentSort::Old);
    assert!(unavailable(&t.banners), "{:?}", t.banners);
}

// ---------------------------------------------------------------------------------------------
// Determinism

/// Two devices that synced the same records at the same time give byte-identical lists,
/// threads and recent discussion; a re-sort answers from the cached layers and agrees with a
/// fresh engine.
#[test]
fn two_engines_give_the_same_bytes() {
    let mut n = Net::new();
    let c = Cast::new(&n);
    let a = n.device("reader1", None);
    let b = n.device("reader2", None);
    let s = scene(&n, &c);
    let f = &s.f.uri;
    sync(&a, f);
    sync(&b, f);
    let bytes = |e: &Engine<MemHost>| -> Vec<String> {
        let mut out = Vec::new();
        for sort in [
            PostSort::Magic,
            PostSort::Top,
            PostSort::New,
            PostSort::Old,
            PostSort::RecentComments,
        ] {
            for filter in [PostFilter::Frontpage, PostFilter::All, PostFilter::Curated] {
                let mut q = ListPostsQuery::new(f);
                q.sort = sort;
                q.filter = filter;
                out.push(serde_json::to_string(&block_on(e.list_posts(&q)).unwrap()).unwrap());
            }
        }
        for sort in [
            CommentSort::Magic,
            CommentSort::Top,
            CommentSort::New,
            CommentSort::Old,
            CommentSort::LatestReply,
        ] {
            out.push(serde_json::to_string(&thread(e, &s.welcome, sort)).unwrap());
        }
        out.push(
            serde_json::to_string(&block_on(e.recent_discussion(f, None, 0)).unwrap()).unwrap(),
        );
        out.push(serde_json::to_string(&block_on(e.mod_log(f, None, 0)).unwrap()).unwrap());
        out
    };
    let x = bytes(&a);
    assert_eq!(x, bytes(&b));
    // asked again (cached model, re-ranked) the answers do not change
    assert_eq!(x, bytes(&a));
}
