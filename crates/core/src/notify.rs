// SPDX-License-Identifier: AGPL-3.0-or-later
//! Notifications and unread state (`social.md` §16), computed locally from one forum's fold and
//! local state. Pure: no I/O and no clock (the engine passes `now`), ordered collections only.
//!
//! **Newness.** An item is new for the viewer when `t_obs(x) > baseline`, where `t_obs` is its
//! first-seen time on this device and `baseline` the greatest `t_obs` already considered for the
//! viewer in this forum. `createdAt` is never used, so backdating cannot hide an item. The first
//! computation for a (viewer, forum) only records the baseline and a snapshot: what was synced
//! before is not news (a new device does not replay a forum's history).
//!
//! **Eligible items** are posts and comments that are listed (`Visible` or `Collapsed`), not by
//! the viewer, and by someone in S ∪ E (the default filter). Each gives at most one
//! notification, for the first matching reason:
//!
//! | Reason | Trigger |
//! |---|---|
//! | Reply | a comment whose parent comment is the viewer's, or a top-level comment on the viewer's post |
//! | Mention | the viewer's DID is among the mentions of the rendered body (`lfm-1.md` §12) |
//! | Quote | a comment with a quote whose target (the parent, else the root) is the viewer's |
//! | Pingback | a post whose rendered links name a post, comment or sequence of the viewer's |
//! | NewComment | a comment on a subscribed post |
//! | NewPost | a post by a subscribed user, in a subscribed forum, or to which a subscribed tag applies |
//!
//! **Changes** are found by diffing a [`Snapshot`] taken at the previous computation:
//! `Reacted` (the supporters of each valid react on the viewer's items grew), `ModAction` (the
//! resolved moderation state of the viewer's items, author-tier effects on their comments, and
//! their own `access`, `restrict` and `nullify`), `KarmaChange` (`K[viewer]`, at most once per
//! cadence period; the difference accumulates meanwhile).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dto::NotificationReason;
use crate::social::full::ForumView;
use crate::social::full::view::{Dim, DimValue, State};
use crate::social::input::{ForumInput, Skel, split_uri};

/// The row version of [`Baseline`].
pub const BASELINE_V: u32 = 1;

/// Previews are cut to this many characters.
pub const PREVIEW_CHARS: usize = 140;

/// What the viewer subscribed to, as it concerns one forum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Subs {
    /// The forum itself (new posts).
    pub forum: bool,
    /// Posts (new comments).
    pub posts: BTreeSet<String>,
    /// Users (their new posts).
    pub users: BTreeSet<String>,
    /// Tag slugs of this forum (new posts they apply to).
    pub tags: BTreeSet<String>,
}

/// The state the change reasons are diffed against.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Valid reacts on the viewer's items: item → `react U+001F qkey` → |supporters|.
    #[serde(default)]
    pub reacts: BTreeMap<String, BTreeMap<String, u32>>,
    /// Moderation state affecting the viewer: `subject U+001F dimension` → value.
    #[serde(default)]
    pub moderation: BTreeMap<String, String>,
    /// `K[viewer]` when last notified.
    #[serde(default)]
    pub karma: i64,
    /// When karma was last notified (ms).
    #[serde(default)]
    pub karma_at_ms: i64,
}

/// The per-(viewer, forum) notification state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    /// Row version.
    pub v: u32,
    /// The greatest `t_obs` (µs) already considered.
    pub obs: i64,
    /// The snapshot of the change reasons.
    #[serde(default)]
    pub snapshot: Snapshot,
}

/// One computed notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// A stable id (an item's notification is `n:<uri>`; a change's carries the time).
    pub id: String,
    /// The reason.
    pub reason: NotificationReason,
    /// First-seen time of the item, or when the change was noticed (ms).
    pub at_ms: i64,
    /// The item, or the forum for changes of the viewer's own standing.
    pub subject: String,
    /// The item's author, or the moderator who decided.
    pub actor: Option<String>,
    /// Plain text.
    pub preview: Option<String>,
}

/// The inputs of one forum's computation.
pub struct Input<'a> {
    /// The viewer's DID.
    pub me: &'a str,
    /// The forum's fold input (skeletons with `t_obs`).
    pub input: &'a ForumInput,
    /// The forum's fold.
    pub view: &'a ForumView,
    /// The viewer's subscriptions in this forum.
    pub subs: &'a Subs,
    /// The record values of the new items ([`new_items`]), for their bodies.
    pub values: &'a BTreeMap<String, Value>,
    /// Now (ms).
    pub now_ms: i64,
    /// The karma cadence (ms; 0 = realtime).
    pub karma_every_ms: i64,
}

/// The time an item was first seen (µs).
pub fn obs(x: &Skel) -> Option<i64> {
    x.times.obs
}

fn listed(view: &ForumView, uri: &str) -> bool {
    view.visibility
        .as_ref()
        .ok()
        .and_then(|v| v.states.get(uri))
        .is_some_and(State::listed)
}

fn is_post(x: &Skel) -> bool {
    x.coll == Some(Collection::Post)
}

fn is_comment(x: &Skel) -> bool {
    x.coll == Some(Collection::Comment)
}

/// The greatest `t_obs` (µs) of the forum's records (0 if none).
pub fn max_obs(input: &ForumInput) -> i64 {
    input.records.values().filter_map(obs).max().unwrap_or(0)
}

/// The eligible items first seen after `baseline` (µs), by uri.
pub fn new_items<'a>(
    input: &'a ForumInput,
    view: &ForumView,
    me: &str,
    baseline: i64,
) -> Vec<&'a Skel> {
    let established: Option<&BTreeSet<String>> = view.karma.as_ref().ok().map(|k| &k.e);
    input
        .records
        .values()
        .filter(|x| is_post(x) || is_comment(x))
        .filter(|x| obs(x).is_some_and(|t| t > baseline))
        .filter(|x| x.did != me)
        .filter(|x| {
            view.participation.s.contains(&x.did) || established.is_some_and(|e| e.contains(&x.did))
        })
        .filter(|x| listed(view, &x.uri))
        .collect()
}

fn author_of(uri: &str) -> Option<&str> {
    split_uri(uri).map(|(d, _, _)| d)
}

fn cut(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut out: String = t.chars().take(PREVIEW_CHARS).collect();
    if t.chars().count() > PREVIEW_CHARS {
        out.push('…');
    }
    Some(out)
}

/// The body source of an inline body (`None`: spilled or absent).
fn body_src(v: &Value) -> Option<&str> {
    v.get("body")?.get("text")?.as_str()
}

/// The reason of a new item, if any (the first matching row of the table).
fn item_reason(
    inp: &Input<'_>,
    x: &Skel,
    meta: Option<&lessdugin_markdown::Metadata>,
) -> Option<NotificationReason> {
    let me = inp.me;
    let refs = x.comment_refs();
    let (root, parent) = refs.unwrap_or((None, None));
    // Reply
    if is_comment(x) {
        let replied = match parent {
            Some(p) => author_of(&p.uri) == Some(me),
            None => root.is_some_and(|r| author_of(&r.uri) == Some(me)),
        };
        if replied {
            return Some(NotificationReason::Reply);
        }
    }
    // Mention
    if meta.is_some_and(|m| m.mentions.iter().any(|d| d.as_str() == me)) {
        return Some(NotificationReason::Mention);
    }
    // Quote
    if let crate::social::input::SkelData::Comment { quote: Some(_), .. } = &x.data {
        let target = parent.or(root);
        if target.is_some_and(|t| author_of(&t.uri) == Some(me)) {
            return Some(NotificationReason::Quote);
        }
    }
    // Pingback
    if is_post(x)
        && meta.is_some_and(|m| {
            m.links.iter().any(|l| {
                split_uri(l.as_str()).is_some_and(|(d, c, _)| {
                    d == me
                        && matches!(
                            Collection::from_nsid(c),
                            Some(Collection::Post | Collection::Comment | Collection::Sequence)
                        )
                })
            })
        })
    {
        return Some(NotificationReason::Pingback);
    }
    // NewComment
    if is_comment(x) && root.is_some_and(|r| inp.subs.posts.contains(&r.uri)) {
        return Some(NotificationReason::NewComment);
    }
    // NewPost
    if is_post(x) {
        let tagged = inp
            .view
            .wiki
            .as_ref()
            .ok()
            .is_some_and(|w| inp.subs.tags.iter().any(|t| w.applies_to(&x.uri, t)));
        if inp.subs.forum || inp.subs.users.contains(&x.did) || tagged {
            return Some(NotificationReason::NewPost);
        }
    }
    None
}

/// The current snapshot of the change reasons.
pub fn snapshot(inp: &Input<'_>) -> Snapshot {
    let me = inp.me;
    let v = inp.view;
    let mine: Vec<&Skel> = inp
        .input
        .records
        .values()
        .filter(|x| x.did == me && x.is_item() && !v.excluded.contains(&x.uri))
        .collect();
    let mut reacts = BTreeMap::new();
    if let Ok(all) = &v.reacts {
        for x in &mine {
            if let Some(ts) = all.get(&x.uri) {
                let m: BTreeMap<String, u32> = ts
                    .iter()
                    .map(|t| {
                        (
                            format!("{}\u{1f}{}", t.react, t.qkey),
                            u32::try_from(t.supporters.len()).unwrap_or(u32::MAX),
                        )
                    })
                    .collect();
                if !m.is_empty() {
                    reacts.insert(x.uri.clone(), m);
                }
            }
        }
    }
    let mut moderation = BTreeMap::new();
    let mut put = |subject: &str, dim: &str, value: String| {
        moderation.insert(format!("{subject}\u{1f}{dim}"), value);
    };
    for x in &mine {
        let u = x.uri.as_str();
        for d in [
            Dim::Hidden,
            Dim::Spam,
            Dim::Tier,
            Dim::PinForum,
            Dim::PinThread,
            Dim::Locked,
        ] {
            if let Some(r) = v.moderation.get(u, d)
                && !neutral(d, &r.value)
            {
                put(u, d.name(), dim_value(&r.value));
            }
        }
        if let Ok(a) = &v.author {
            if a.moderated.contains(u) {
                put(u, "author", "moderated".into());
            }
            if let Some(p) = a.thread_pins.get(u) {
                put(u, "author.pin", p.to_string());
            }
            if a.locks.contains_key(u) {
                put(u, "author.lock", "true".into());
            }
        }
    }
    for d in [Dim::Access, Dim::Restrict, Dim::Nullify] {
        if let Some(r) = v.moderation.get(me, d)
            && !neutral(d, &r.value)
        {
            put(me, d.name(), dim_value(&r.value));
        }
    }
    let karma = v.karma.as_ref().map_or(0, |k| k.k_of(me));
    Snapshot {
        reacts,
        moderation,
        karma,
        karma_at_ms: 0,
    }
}

/// `true` for a resolved value that changes nothing a user would notice: `false` of every
/// boolean dimension but `hidden` (where it is a keep), e.g. the `spam: false` every hide
/// carries.
fn neutral(d: Dim, v: &DimValue) -> bool {
    matches!(v, DimValue::Bool(false)) && d != Dim::Hidden
}

fn dim_value(v: &DimValue) -> String {
    match v {
        DimValue::Bool(b) => b.to_string(),
        DimValue::Access(a) => a.as_str().to_owned(),
        DimValue::Tier(t) => t.as_str().to_owned(),
        DimValue::Caps(c) => c.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(","),
        DimValue::Head { uri, .. } => uri.clone(),
    }
}

/// A human description of a moderation change (`None` → `value`).
fn mod_preview(dim: &str, value: Option<&str>) -> String {
    match (dim, value) {
        ("hidden", Some("true")) => "hidden by a moderator".into(),
        ("hidden", Some("false")) => "kept visible by a moderator".into(),
        ("hidden", None) => "no longer hidden".into(),
        ("spam", Some("true")) => "marked as spam".into(),
        ("spam", _) => "no longer marked as spam".into(),
        ("tier", Some(t)) => format!("moved to {t}"),
        ("tier", None) => "back to the forum's default tier".into(),
        ("pin.forum" | "pin.thread" | "author.pin", Some("true")) => "pinned".into(),
        ("pin.forum" | "pin.thread" | "author.pin", _) => "unpinned".into(),
        ("locked" | "author.lock", Some("true")) => "locked".into(),
        ("locked" | "author.lock", _) => "unlocked".into(),
        ("author", Some(_)) => "hidden by the post's author".into(),
        ("author", None) => "no longer hidden by the post's author".into(),
        ("access", Some("banned")) => "you were banned from this forum".into(),
        ("access", Some("member")) => "you were approved as a member".into(),
        ("access", _) => "your access changed".into(),
        ("restrict", Some("")) | ("restrict", None) => "your restrictions were lifted".into(),
        ("restrict", Some(c)) => format!("you were restricted from: {c}"),
        ("nullify", Some("true")) => "your votes were nullified".into(),
        ("nullify", _) => "your votes count again".into(),
        (d, Some(v)) => format!("{d}: {v}"),
        (d, None) => format!("{d} lifted"),
    }
}

/// The moderator behind a resolved dimension (the winner's actor).
fn mod_actor(view: &ForumView, key: &str) -> Option<String> {
    let (subject, dim) = key.split_once('\u{1f}')?;
    if dim.starts_with("author") {
        let root = subject;
        return author_of(root).map(str::to_owned);
    }
    let d = Dim::ALL.into_iter().find(|d| d.name() == dim)?;
    let r = view.moderation.get(subject, d)?;
    author_of(&r.winner).map(str::to_owned)
}

/// One forum's computation: the notifications since `base`, and the next baseline. Without a
/// previous baseline (`base = None`) nothing is notified: the baseline and the snapshot are
/// only recorded.
pub fn compute(inp: &Input<'_>, base: Option<&Baseline>) -> (Vec<Note>, Baseline) {
    let now = inp.now_ms;
    let snap = snapshot(inp);
    let top = max_obs(inp.input);
    let Some(base) = base else {
        let mut snapshot = snap;
        snapshot.karma_at_ms = now;
        return (
            Vec::new(),
            Baseline {
                v: BASELINE_V,
                obs: top,
                snapshot,
            },
        );
    };
    let mut notes = Vec::new();
    // new items
    for x in new_items(inp.input, inp.view, inp.me, base.obs) {
        let value = inp.values.get(&x.uri);
        let meta = value
            .and_then(body_src)
            .map(|src| lessdugin_markdown::derive_metadata(src, &[]));
        let Some(reason) = item_reason(inp, x, meta.as_ref()) else {
            continue;
        };
        let title = match &x.data {
            crate::social::input::SkelData::Post { title, .. } => title.clone(),
            _ => None,
        };
        let preview = title
            .as_deref()
            .and_then(cut)
            .or_else(|| meta.as_ref().and_then(|m| cut(&m.excerpt)));
        notes.push(Note {
            id: format!("n:{}", x.uri),
            reason,
            at_ms: obs(x).unwrap_or(0).div_euclid(1000),
            subject: x.uri.clone(),
            actor: Some(x.did.clone()),
            preview,
        });
    }
    let old = &base.snapshot;
    // reacts: per item, the reacts whose supporters grew
    for (item, now_counts) in &snap.reacts {
        let before = old.reacts.get(item);
        let grown: Vec<String> = now_counts
            .iter()
            .filter_map(|(k, n)| {
                let was = before.and_then(|b| b.get(k)).copied().unwrap_or(0);
                (*n > was).then(|| {
                    let (react, q) = k.split_once('\u{1f}').unwrap_or((k, ""));
                    let on = if q.is_empty() {
                        String::new()
                    } else {
                        format!(" on “{q}”")
                    };
                    format!("{react}{on} +{}", n - was)
                })
            })
            .collect();
        if !grown.is_empty() {
            notes.push(Note {
                id: format!("r:{item}:{now}"),
                reason: NotificationReason::Reacted,
                at_ms: now,
                subject: item.clone(),
                actor: None,
                preview: cut(&grown.join(", ")),
            });
        }
    }
    // moderation: every key whose value changed, appeared or disappeared
    let keys: BTreeSet<&String> = snap
        .moderation
        .keys()
        .chain(old.moderation.keys())
        .collect();
    for k in keys {
        let (a, b) = (old.moderation.get(k), snap.moderation.get(k));
        if a == b {
            continue;
        }
        let (subject, dim) = k.split_once('\u{1f}').unwrap_or((k, ""));
        let subject = if subject == inp.me {
            inp.input.forum.clone()
        } else {
            subject.to_owned()
        };
        notes.push(Note {
            id: format!("m:{k}:{now}"),
            reason: NotificationReason::ModAction,
            at_ms: now,
            subject,
            actor: if b.is_some() {
                mod_actor(inp.view, k)
            } else {
                None
            },
            preview: Some(mod_preview(dim, b.map(String::as_str))),
        });
    }
    // karma, at most once per cadence period
    let mut next = snap.clone();
    next.karma_at_ms = old.karma_at_ms;
    if snap.karma != old.karma {
        if now.saturating_sub(old.karma_at_ms) >= inp.karma_every_ms {
            let d = snap.karma - old.karma;
            notes.push(Note {
                id: format!("k:{}:{now}", inp.input.forum),
                reason: NotificationReason::KarmaChange,
                at_ms: now,
                subject: inp.input.forum.clone(),
                actor: None,
                preview: Some(format!(
                    "{}{d} karma in this forum (now {})",
                    if d > 0 { "+" } else { "" },
                    snap.karma
                )),
            });
            next.karma_at_ms = now;
        } else {
            // not due yet: the difference accumulates
            next.karma = old.karma;
        }
    }
    (
        notes,
        Baseline {
            v: BASELINE_V,
            obs: top.max(base.obs),
            snapshot: next,
        },
    )
}

/// The listed comments of `post`'s thread first seen after `mark` (µs), not by `me`
/// (`social.md` §16 read state: a comment is unread if `t_obs(c) > lastVisited[root]`).
pub fn unread_comments(
    input: &ForumInput,
    view: &ForumView,
    post: &str,
    mark: i64,
    me: Option<&str>,
) -> u32 {
    let n = input
        .records
        .values()
        .filter(|x| is_comment(x))
        .filter(|x| {
            x.comment_refs()
                .and_then(|(r, _)| r)
                .is_some_and(|r| r.uri == post)
        })
        .filter(|x| Some(x.did.as_str()) != me)
        .filter(|x| obs(x).is_some_and(|t| t > mark))
        .filter(|x| listed(view, &x.uri))
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The listed posts first seen after `mark` (µs), not by `me`.
pub fn unread_posts(input: &ForumInput, view: &ForumView, mark: i64, me: Option<&str>) -> u32 {
    let n = input
        .records
        .values()
        .filter(|x| is_post(x))
        .filter(|x| Some(x.did.as_str()) != me)
        .filter(|x| obs(x).is_some_and(|t| t > mark))
        .filter(|x| listed(view, &x.uri))
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The greatest `t_obs` (µs) of `post` and the listed comments of its thread (what viewing it
/// marks read), if any.
pub fn thread_obs(input: &ForumInput, view: &ForumView, post: &str) -> Option<i64> {
    input
        .records
        .values()
        .filter(|x| {
            x.uri == post
                || (is_comment(x)
                    && x.comment_refs()
                        .and_then(|(r, _)| r)
                        .is_some_and(|r| r.uri == post)
                    && listed(view, &x.uri))
        })
        .filter_map(obs)
        .max()
}

/// The greatest `t_obs` (µs) of the forum's listed posts, if any.
pub fn forum_obs(input: &ForumInput, view: &ForumView) -> Option<i64> {
    input
        .records
        .values()
        .filter(|x| is_post(x) && listed(view, &x.uri))
        .filter_map(obs)
        .max()
}
