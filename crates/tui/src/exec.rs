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
    ActorView, Capability, ListPostsQuery, ModActionIntent, ModActionValue, ModScope,
    NotificationsQuery, PostFilter, PostSort, Ref, RefContext, SyncOpts, ThreadOpts, ThreadRoot,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_core::host::Host;
use lessdugin_core::store::records::RecordStore;

use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::ns::NS;

use crate::app::{Action, ActivityData, Effect, ForumData, HomeData, SyncTarget, ThreadData};
use crate::body::Body;
use crate::command::RestrictDim;

/// How many queued handle verifications a load runs before showing a view (as the CLI).
pub const VERIFY_HANDLES: usize = 256;

/// The most posts a forum screen lists.
pub const LIST_LIMIT: u32 = 200;

/// The page the parity scenario compares (`docs/plan.md` §4.2): the CLI's `ls -n` default and
/// the SPA's page size. The Forum screen lists up to [`LIST_LIMIT`]; its first posts are this
/// page.
pub const PARITY_LIST_LIMIT: u32 = 30;

/// The most mod-log entries the Mod screen lists (`mod_log` pages; the first page is enough
/// for the MVP).
pub const MOD_LOG_LIMIT: u32 = 100;

/// The most notifications the Notifications screen lists.
pub const NOTIF_LIMIT: u32 = 100;

/// The most Recent Discussion items the Forum screen's Activity tab lists.
pub const ACTIVITY_LIMIT: u32 = 50;

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
            filter,
            origin,
        } => Action::ForumLoaded {
            screen,
            origin,
            result: load_forum(engine, &forum, sort, filter, opts)
                .await
                .map(Box::new),
        },
        Effect::LoadThread {
            screen,
            uri,
            opts: thread_opts,
            origin,
        } => Action::ThreadLoaded {
            screen,
            origin,
            result: load_thread(engine, &uri, &thread_opts, opts)
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
        Effect::Opinion {
            subject,
            intent,
            forum,
            screen,
            label,
        } => Action::OpinionDone {
            result: match item_uri(engine, &subject, &forum).await {
                Ok(uri) => engine.opinion(&uri, &intent).await,
                Err(e) => Err(e),
            },
            forum,
            screen,
            label,
        },
        Effect::Undo { id, screen } => Action::Undone {
            screen,
            result: engine.undo(&id).await,
        },
        Effect::Grant {
            forum,
            actors,
            screen,
        } => Action::Granted {
            screen,
            result: engine.grant(&forum, &actors).await,
        },
        Effect::ModsSet {
            forum,
            change,
            screen,
        } => Action::ModsSetDone {
            screen,
            result: engine.mods_set(&forum, &change).await,
        },
        Effect::ModAction {
            intent,
            forum,
            screen,
            merge,
        } => Action::ModActionDone {
            forum,
            screen,
            result: match mod_intent(engine, intent, merge).await {
                Ok(intent) => engine.mod_action(&intent).await,
                Err(e) => Err(e),
            },
        },
        Effect::LoadQueue {
            screen,
            forum,
            origin,
        } => Action::QueueLoaded {
            screen,
            origin,
            result: engine.mod_queue(&forum).await,
        },
        Effect::LoadLog {
            screen,
            forum,
            origin,
        } => Action::LogLoaded {
            screen,
            origin,
            result: engine.mod_log(&forum, None, MOD_LOG_LIMIT).await,
        },
        Effect::LoadActivity {
            screen,
            forum,
            origin,
        } => Action::ActivityLoaded {
            screen,
            origin,
            result: load_activity(engine, &forum, opts).await.map(Box::new),
        },
        Effect::LoadHistory {
            screen,
            uri,
            origin,
        } => Action::HistoryLoaded {
            screen,
            origin,
            // the panel shows why it could not load rather than staying empty
            result: engine.history(&uri).await.map_err(|e| e.to_string()),
        },
        Effect::LoadNotifications {
            screen,
            all,
            origin,
        } => Action::NotificationsLoaded {
            screen,
            origin,
            result: engine
                .notifications(&NotificationsQuery {
                    all,
                    limit: NOTIF_LIMIT,
                })
                .await,
        },
        Effect::MarkRead { ids, screen } => Action::MarkedRead {
            screen,
            result: engine.mark_notifications_read(&ids).await,
        },
        Effect::LoadUnread => Action::UnreadLoaded(engine.unread_counts().await),
        Effect::MarkThreadRead { post } => {
            // a read mark that could not be stored only leaves the thread unread
            if let Err(e) = engine.mark_read(&post).await {
                tracing::debug!(error = %e, %post, "mark_read failed");
            }
            return None;
        }
        Effect::LoadWrites => Action::WritesLoaded(engine.writes().await),
        Effect::Editor(_) | Effect::Quit => return None,
    })
}

/// The at-uri of an item a command line named: a canonical uri as it is, anything else through
/// `parse_ref` (an SPA link, an at-uri with a handle, `handle/rkey` in `forum`).
pub async fn item_uri<H: Host>(
    engine: &Engine<H>,
    subject: &str,
    forum: &str,
) -> Result<String, EngineError> {
    if subject.starts_with("at://did:") {
        return Ok(subject.to_owned());
    }
    let ctx = RefContext {
        forum: (!forum.is_empty()).then(|| forum.to_owned()),
    };
    match engine.parse_ref(subject, &ctx).await? {
        Ref::Post { uri } | Ref::Comment { uri } | Ref::Sequence { uri } | Ref::Record { uri } => {
            Ok(uri)
        }
        Ref::Forum { .. } | Ref::Actor { .. } => Err(EngineError::invalid_ref(format!(
            "{subject} is not a post or a comment"
        ))),
    }
}

/// The intent as the engine takes it: an item subject named by any reference resolved to its
/// uri, and a `#restrict` assertion that sets one dimension completed with the other from the
/// account's current record (a record states the whole state of its key, `social.md` §4.2).
pub async fn mod_intent<H: Host>(
    engine: &Engine<H>,
    mut intent: ModActionIntent,
    merge: Option<RestrictDim>,
) -> Result<ModActionIntent, EngineError> {
    let (ModActionIntent::Assert { forum, subject, .. }
    | ModActionIntent::Remove { forum, subject, .. }) = &mut intent;
    // a DID, a handle and `tag:` are the engine's; an item named by a link is resolved here
    let item_ref = subject.starts_with("at://") || subject.contains("#/");
    if item_ref && !subject.starts_with("at://did:") {
        *subject = item_uri(engine, subject, forum).await?;
    }
    let (
        Some(dim),
        ModActionIntent::Assert {
            forum,
            scope,
            subject,
            action:
                ModActionValue::Restrict {
                    capabilities,
                    nullify_votes,
                },
            ..
        },
    ) = (merge, &mut intent)
    else {
        return Ok(intent);
    };
    let (held_caps, held_nullify) = held_restrict(engine, forum, *scope, subject).await?;
    match dim {
        RestrictDim::Capabilities => *nullify_votes = held_nullify,
        RestrictDim::Nullify => *capabilities = held_caps,
    }
    Ok(intent)
}

/// The account's own `#restrict` record on `subject` in `forum` (one per scope, subject and
/// key, `data-model.md` §6.7) as `(capabilities, nullifyVotes)`; nothing when it has none.
async fn held_restrict<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    scope: ModScope,
    subject: &str,
) -> Result<(Vec<Capability>, bool), EngineError> {
    let Some(me) = engine.active_account().await?.map(|a| a.did) else {
        return Ok((Vec::new(), false));
    };
    let did = if subject.starts_with("did:") {
        subject.to_owned()
    } else {
        engine.resolve_actor(subject).await?.did
    };
    let forum = match engine.parse_ref(forum, &RefContext::default()).await? {
        Ref::Forum { uri } => uri,
        _ => forum.to_owned(),
    };
    let mut action = serde_json::Map::new();
    action.insert(
        "$type".into(),
        serde_json::Value::String(nsid::MOD_ACTION_RESTRICT.to_owned()),
    );
    let scope = match scope {
        ModScope::Forum => None,
        ModScope::Author => Some("author"),
    };
    let rkey = lessdugin_lexicon::keys::mod_action_rkey(NS, &forum, scope, &did, &action)
        .map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let Ok(uri) = AtUri::parse(&format!("at://{me}/{}/{rkey}", nsid::MOD_ACTION)) else {
        return Ok((Vec::new(), false));
    };
    let value = match RecordStore::new(engine.host().cache()).get(&uri).await {
        Ok(Some(row)) => row.value_json().ok(),
        _ => None,
    };
    let Some(action) = value.as_ref().and_then(|v| v.get("action")) else {
        return Ok((Vec::new(), false));
    };
    let caps = action
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| serde_json::from_value(c.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let nullify = action
        .get("nullifyVotes")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok((caps, nullify))
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

/// `forum_open` and `list_posts`.
pub async fn load_forum<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    sort: PostSort,
    filter: PostFilter,
    opts: ExecOpts,
) -> Result<ForumData, EngineError> {
    let view = engine.forum_open(forum).await?;
    let q = posts_query(forum, sort, filter, LIST_LIMIT);
    let mut list = engine.list_posts(&q).await?;
    if !opts.offline
        && unverified(list.items.iter().map(|p| &p.author))
        && engine.verify_handles(VERIFY_HANDLES).await > 0
    {
        list = engine.list_posts(&q).await?;
    }
    Ok(ForumData { view, list })
}

/// `recent_discussion(forum, None, ACTIVITY_LIMIT)` and the bodies of the comments it shows
/// (`social.md` §12.5; the Forum screen's Activity tab, `synthesis-v1.md` §6.11).
pub async fn load_activity<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    opts: ExecOpts,
) -> Result<ActivityData, EngineError> {
    let mut list = engine
        .recent_discussion(forum, None, ACTIVITY_LIMIT)
        .await?;
    let actors: Vec<&ActorView> = list
        .items
        .iter()
        .flat_map(|i| std::iter::once(&i.post.author).chain(i.comments.iter().map(|c| &c.author)))
        .collect();
    if !opts.offline
        && unverified(actors.into_iter())
        && engine.verify_handles(VERIFY_HANDLES).await > 0
    {
        list = engine
            .recent_discussion(forum, None, ACTIVITY_LIMIT)
            .await?;
    }
    let mut bodies = BTreeMap::new();
    for item in &list.items {
        for c in &item.comments {
            let b = body(engine, &c.uri, &c.cid, &c.body).await;
            bodies.insert(c.uri.clone(), b);
        }
    }
    Ok(ActivityData { list, bodies })
}

/// The query of the Forum screen in `sort` and `filter` (it opens on the M3 defaults, `magic`
/// over `frontpage`, `engine-api.md` §3.4), at most `limit` posts.
pub fn posts_query(forum: &str, sort: PostSort, filter: PostFilter, limit: u32) -> ListPostsQuery {
    ListPostsQuery {
        forum: forum.to_owned(),
        sort,
        filter,
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
