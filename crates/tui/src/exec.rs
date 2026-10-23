// SPDX-License-Identifier: AGPL-3.0-or-later
//! Running the engine effects: each [`Effect`] that calls the engine becomes one future whose
//! result is an [`Action`]. The runtime spawns them as tasks; the headless driver awaits them
//! in order.
//!
//! Loads read the local cache (`engine-api.md` §3.4); like the CLI, a view whose authors have
//! no verified handle yet runs the queued handle checks first and is queried again, so it shows
//! `@handle` rather than a short DID. Bodies come from the cached records (`crate::body`).

use std::collections::BTreeMap;

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::dto::{
    ActorView, ListPostsQuery, PostFilter, PostSort, SyncOpts, ThreadOpts, ThreadRoot,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_core::host::Host;
use lessdugin_core::store::records::RecordStore;

use crate::app::{Action, Effect, ForumData, HomeData, SyncTarget, ThreadData};
use crate::body::Body;

/// How many queued handle verifications a load runs before showing a view (as the CLI).
pub const VERIFY_HANDLES: usize = 256;

/// The most posts a forum screen lists.
pub const LIST_LIMIT: u32 = 200;

/// The page the parity scenario compares (`docs/plan.md` §4.2): the CLI's `ls -n` default and
/// the SPA's page size. The Forum screen lists up to [`LIST_LIMIT`]; its first posts are this
/// page.
pub const PARITY_LIST_LIMIT: u32 = 30;

/// The time budget of the syncs a screen runs when it opens (as the CLI's implicit syncs).
pub const IMPLICIT_SYNC_MS: i64 = 20_000;

/// Options of the effect runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExecOpts {
    /// No network: skip handle verification.
    pub offline: bool,
}

/// Runs an engine effect; `None` for the effects the runtime handles itself (`Editor`,
/// `Quit`).
pub async fn run<H: Host>(engine: &Engine<H>, effect: Effect, opts: ExecOpts) -> Option<Action> {
    Some(match effect {
        Effect::LoadHome { screen, origin } => Action::HomeLoaded {
            screen,
            origin,
            result: load_home(engine, opts).await,
        },
        Effect::LoadForum {
            screen,
            forum,
            sort,
            origin,
        } => Action::ForumLoaded {
            screen,
            origin,
            result: load_forum(engine, &forum, sort, opts).await.map(Box::new),
        },
        Effect::LoadThread {
            screen,
            uri,
            origin,
        } => Action::ThreadLoaded {
            screen,
            origin,
            result: load_thread(engine, &uri, &ThreadOpts::default(), opts)
                .await
                .map(Box::new),
        },
        Effect::Sync { target, user } => {
            let results = sync(engine, &target, user).await;
            Action::Synced {
                target,
                user,
                at_ms: engine.host().now_ms(),
                results,
            }
        }
        Effect::Comment { draft } => Action::Queued {
            compose: crate::app::Compose::Comment {
                root: draft.root.clone(),
                parent: draft.parent.clone(),
            },
            text: draft.body.markdown.clone(),
            result: engine.comment(&draft).await,
        },
        Effect::Publish { draft } => Action::Queued {
            compose: crate::app::Compose::Post {
                forum: draft.forum.clone(),
            },
            text: match &draft.title {
                Some(t) => format!("{t}\n\n{}", draft.body.markdown),
                None => draft.body.markdown.clone(),
            },
            result: engine.publish(&draft).await,
        },
        Effect::LoadWrites => Action::WritesLoaded(engine.writes().await),
        Effect::Editor(_) | Effect::Quit => return None,
    })
}

fn unverified<'a>(mut actors: impl Iterator<Item = &'a ActorView>) -> bool {
    actors.any(|a| a.handle.is_none())
}

/// `followed()`.
pub async fn load_home<H: Host>(
    engine: &Engine<H>,
    opts: ExecOpts,
) -> Result<HomeData, EngineError> {
    let mut forums = engine.followed().await?;
    if !opts.offline
        && unverified(forums.iter().map(|f| &f.owner))
        && engine.verify_handles(VERIFY_HANDLES).await > 0
    {
        forums = engine.followed().await?;
    }
    Ok(HomeData { forums })
}

/// `forum_open` and `list_posts` (filter all).
pub async fn load_forum<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    sort: PostSort,
    opts: ExecOpts,
) -> Result<ForumData, EngineError> {
    let view = engine.forum_open(forum).await?;
    let q = posts_query(forum, sort, LIST_LIMIT);
    let mut list = engine.list_posts(&q).await?;
    if !opts.offline
        && unverified(list.items.iter().map(|p| &p.author))
        && engine.verify_handles(VERIFY_HANDLES).await > 0
    {
        list = engine.list_posts(&q).await?;
    }
    Ok(ForumData { view, list })
}

/// The query of the Forum screen (`filter: all`, the M2 default of every client,
/// `engine-api.md` §3.4), at most `limit` posts.
pub fn posts_query(forum: &str, sort: PostSort, limit: u32) -> ListPostsQuery {
    ListPostsQuery {
        forum: forum.to_owned(),
        sort,
        filter: PostFilter::All,
        tag: None,
        kind: None,
        author: None,
        cursor: None,
        limit,
    }
}

/// `get_thread` and the bodies of its items.
pub async fn load_thread<H: Host>(
    engine: &Engine<H>,
    uri: &str,
    thread_opts: &ThreadOpts,
    opts: ExecOpts,
) -> Result<ThreadData, EngineError> {
    let mut view = engine.get_thread(uri, thread_opts).await?;
    let actors = |v: &lessdugin_core::dto::ThreadView| -> Vec<ActorView> {
        let mut a: Vec<ActorView> = v.comments.iter().map(|c| c.author.clone()).collect();
        if let ThreadRoot::Post(p) = &v.post {
            a.push(p.post.author.clone());
        }
        a
    };
    if !opts.offline
        && unverified(actors(&view).iter())
        && engine.verify_handles(VERIFY_HANDLES).await > 0
    {
        view = engine.get_thread(uri, thread_opts).await?;
    }
    let mut bodies = BTreeMap::new();
    if let ThreadRoot::Post(p) = &view.post {
        let b = body(engine, &p.post.uri, &p.post.cid, &p.body).await;
        bodies.insert(p.post.uri.clone(), b);
    }
    for c in &view.comments {
        let b = body(engine, &c.uri, &c.cid, &c.body).await;
        bodies.insert(c.uri.clone(), b);
    }
    Ok(ThreadData { view, bodies })
}

/// The body of `uri` as the view shows it: the cached record's markdown when its CID is
/// `cid`, else the view's text.
async fn body<H: Host>(
    engine: &Engine<H>,
    uri: &str,
    cid: &str,
    view: &lessdugin_core::dto::RenderedBody,
) -> Body {
    let record = match AtUri::parse(uri) {
        Ok(u) => match RecordStore::new(engine.host().cache()).get(&u).await {
            Ok(Some(row)) if row.cid.to_string() == cid => row.value_json().ok(),
            _ => None,
        },
        Err(_) => None,
    };
    Body::from_record(record.as_ref(), view)
}

/// Runs a sync; the outcome per forum (or per item when its forum is not known).
pub async fn sync<H: Host>(
    engine: &Engine<H>,
    target: &SyncTarget,
    user: bool,
) -> Vec<(String, Result<lessdugin_core::dto::SyncReport, EngineError>)> {
    let opts = SyncOpts {
        budget_ms: (!user).then_some(IMPLICIT_SYNC_MS),
        max_requests: None,
        full: false,
    };
    let mut out = Vec::new();
    match target {
        SyncTarget::Forums(forums) => {
            for f in forums {
                out.push((f.clone(), engine.sync_forum(f, &opts).await));
            }
        }
        SyncTarget::Forum(f) => out.push((f.clone(), engine.sync_forum(f, &opts).await)),
        SyncTarget::Thread { uri, forum } => {
            let r = engine.sync_thread(uri, &opts).await;
            let key = match &r {
                Ok(rep) => rep.forum.clone(),
                Err(_) => forum.clone().unwrap_or_else(|| uri.clone()),
            };
            out.push((key, r));
            if let Some(f) = forum {
                out.push((f.clone(), engine.sync_forum(f, &opts).await));
            }
        }
    }
    out
}
