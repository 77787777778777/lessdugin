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
    ActionNotice, ActorView, BodyDraft, Capability, CommentDraft, DefaultTier, Draft, EditOpts,
    ExternalLinkDraft, ForumDraft, KarmaCadence, ListPostsQuery, MembersQuery, MembershipMode,
    ModActionIntent, ModActionValue, ModScope, NotificationsQuery, PostDraft, PostFilter, PostKind,
    PostSort, Ref, RefContext, SyncOpts, TextQuote, ThreadOpts, ThreadRoot,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_core::host::Host;
use lessdugin_core::outbox::model::OpAction;
use lessdugin_core::store::records::RecordStore;

use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::ns::NS;

use crate::app::{
    Action, ActivityData, Compose, EditorRequest, Effect, ForumData, HomeData, ItemOp, SyncTarget,
    ThreadData,
};
use crate::body::Body;
use crate::command::{ALL_CAPABILITIES, AccessOp, ForumChanges, RestrictDim};

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
        Effect::MarkAllRead { screen } => Action::MarkedRead {
            screen,
            result: engine.mark_all_notifications_read().await,
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
        Effect::LoadAcknowledged => match engine.active_account().await {
            Ok(account) => Action::AcknowledgedLoaded {
                account: account.map(|a| a.did),
                result: engine.acknowledged_public_actions().await,
            },
            Err(e) => Action::AcknowledgedLoaded {
                account: None,
                result: Err(e),
            },
        },
        Effect::Acknowledge { kind } => {
            // not stored: the next session asks again; this one already let the write go
            if let Err(e) = engine.acknowledge_public_action(kind).await {
                tracing::debug!(error = %e, ?kind, "acknowledge_public_action failed");
            }
            return None;
        }
        Effect::Open { target } => Action::Opened(
            match engine.parse_ref(&target, &RefContext::default()).await {
                Ok(r @ (Ref::Forum { .. } | Ref::Post { .. } | Ref::Comment { .. })) => Ok(r),
                // a forum reference parse_ref does not take (`owner/slug`)
                other => match engine.forum_open(&target).await {
                    Ok(v) => Ok(Ref::Forum { uri: v.forum.uri }),
                    Err(_) => other,
                },
            },
        ),
        Effect::Follow { forum, on } => Action::LocalDone(if on {
            engine
                .forum_follow(&forum, None)
                .await
                .map(|()| "Following the forum.".to_owned())
        } else {
            engine
                .forum_unfollow(&forum)
                .await
                .map(|()| "No longer following the forum.".to_owned())
        }),
        Effect::Join {
            forum,
            note,
            screen,
        } => Action::Written {
            what: "Join",
            screen: Some(screen),
            result: engine.join(&forum, note.as_deref()).await.map(|t| vec![t]),
        },
        Effect::Leave { forum, screen } => Action::Written {
            what: "Leave",
            screen: Some(screen),
            result: engine.leave(&forum).await.map(|t| vec![t]),
        },
        Effect::LoadMembers {
            forum,
            status,
            pending,
        } => Action::MembersLoaded(
            engine
                .members(
                    &forum,
                    &MembersQuery {
                        status,
                        pending: pending.then_some(true),
                    },
                )
                .await,
        ),
        Effect::Access {
            op,
            forum,
            actors,
            screen,
        } => {
            let (what, result) = match op {
                AccessOp::Grant => ("Grant", engine.grant(&forum, &actors).await),
                AccessOp::Revoke => ("Revoke", engine.revoke(&forum, &actors).await),
                AccessOp::Approve => ("Approve", engine.approve(&forum, &actors).await),
                AccessOp::Reject => ("Reject", engine.reject(&forum, &actors).await),
            };
            Action::Written {
                what,
                screen: Some(screen),
                result,
            }
        }
        Effect::Adopt {
            forum,
            former,
            screen,
        } => Action::Written {
            what: "Adoption",
            screen: Some(screen),
            result: engine.mod_adopt(&forum, &former).await.map(|t| vec![t]),
        },
        Effect::ForumCreate { draft } => Action::Written {
            what: if draft.membership == MembershipMode::Open {
                crate::app::NEW_OPEN_FORUM
            } else {
                "New forum"
            },
            screen: None,
            result: engine.forum_create(&draft).await.map(|t| vec![t]),
        },
        Effect::ForumEdit {
            forum,
            changes,
            screen,
        } => Action::Written {
            what: if changes.membership == Some(MembershipMode::Open) {
                crate::app::OPEN_FORUM_EDIT
            } else {
                "Forum edit"
            },
            screen: Some(screen),
            result: match forum_draft(engine, &forum, &changes).await {
                Ok(d) => engine.forum_update(&forum, &d).await.map(|t| vec![t]),
                Err(e) => Err(e),
            },
        },
        Effect::StartEdit { uri, forum } => {
            Action::EditReady(start_edit(engine, &uri, &forum).await)
        }
        Effect::Edit { uri, draft, text } => Action::Queued {
            compose: Compose::Edit {
                uri: uri.clone(),
                base: Box::new(draft.clone()),
            },
            text,
            result: engine.edit(&uri, &draft, &EditOpts::default()).await,
        },
        Effect::ItemWrite {
            op,
            uri,
            forum,
            screen,
        } => {
            let what = match op {
                ItemOp::Delete => "Delete",
                ItemOp::Retract => "Retract",
                ItemOp::Redact => "Redaction",
            };
            let result = match item_uri(engine, &uri, &forum).await {
                Ok(uri) => match op {
                    ItemOp::Delete => engine.delete(&uri).await.map(|t| vec![t]),
                    ItemOp::Retract => engine.retract(&uri).await.map(|t| vec![t]),
                    ItemOp::Redact => engine.redact_revision(&uri).await,
                },
                Err(e) => Err(e),
            };
            Action::Written {
                what,
                screen: Some(screen),
                result,
            }
        }
        Effect::LoadRaw { uri, forum } => {
            Action::RawLoaded(match item_uri(engine, &uri, &forum).await {
                Ok(uri) => engine.raw(&uri).await,
                Err(e) => Err(e),
            })
        }
        Effect::Bookmark { uri, forum, on } => {
            Action::LocalDone(match item_uri(engine, &uri, &forum).await {
                Ok(uri) => engine.bookmark(&uri, on).await.map(|()| {
                    if on {
                        "Bookmarked."
                    } else {
                        "Bookmark removed."
                    }
                    .to_owned()
                }),
                Err(e) => Err(e),
            })
        }
        Effect::Subscribe { subscription, on } => Action::LocalDone(
            engine
                .set_subscription(&subscription, on)
                .await
                .map(|()| if on { "Subscribed." } else { "Unsubscribed." }.to_owned()),
        ),
        Effect::Mute { actor, on } => Action::LocalDone(
            engine
                .mute(&actor, on)
                .await
                .map(|()| if on { "Muted." } else { "Unmuted." }.to_owned()),
        ),
        Effect::KarmaNotifications { cadence } => Action::LocalDone(match cadence {
            Some(c) => engine
                .set_karma_notifications(c)
                .await
                .map(|()| format!("Karma notifications: {}.", cadence_name(c))),
            None => engine
                .karma_notifications()
                .await
                .map(|c| format!("Karma notifications: {}.", cadence_name(c))),
        }),
        Effect::Settings { set } => {
            if let Some((key, value)) = set
                && let Err(e) = engine
                    .set_setting(&key, value.as_deref().map(setting_value).as_ref())
                    .await
            {
                return Some(Action::LocalDone(Err(e)));
            }
            Action::SettingsLoaded(engine.settings().await)
        }
        Effect::Retry { id } => Action::Written {
            what: "Retry",
            screen: None,
            result: engine.retry_write(&id).await.map(|t| vec![t]),
        },
        Effect::FetchMedia { media } => Action::MediaFetched {
            result: fetch_media(engine, &media).await,
            cid: media.cid,
        },
        Effect::Editor(_) | Effect::DecodeMedia { .. } | Effect::Quit => return None,
    })
}

/// The bytes of the image `media`, for the decoder: `load_media(cid, media)` fetches it from
/// its AUTHOR's PDS (resolved from their DID document, `sync.md` §11.1), streamed and
/// CID-checked by the host, at most its declared size (a blob the host already holds is not
/// fetched again); it is then read back from the host's store a window at a time. An image
/// declared past the decoder's input cap, or of no declared size, is not fetched at all, and one
/// whose content is not the image type it declares is not decoded (`mime` absent: the core's
/// display allowlist, §11.2).
pub async fn fetch_media<H: Host>(
    engine: &Engine<H>,
    media: &lessdugin_core::dto::MediaView,
) -> Result<crate::image::MediaBytes, EngineError> {
    use lessdugin_core::engine::ErrorCode;
    let max = crate::image::limits::MAX_INPUT_BYTES as u64;
    if media.size_bytes == 0 || media.size_bytes > max {
        return Err(EngineError::new(
            ErrorCode::TooLarge,
            "the image is over the 64 MiB a terminal decodes, or states no size",
        ));
    }
    let loaded = engine
        .load_media(
            &media.cid,
            Some(lessdugin_core::media::MediaOwner::of_view(media)),
        )
        .await?;
    if !loaded
        .mime
        .as_deref()
        .is_some_and(crate::thread::inline_image)
    {
        return Err(EngineError::invalid_input(
            "the file is not the image it says it is",
        ));
    }
    if loaded.size > max {
        return Err(EngineError::new(
            ErrorCode::TooLarge,
            "the image is over the 64 MiB a terminal decodes",
        ));
    }
    // (a task of the runtime, not the UI thread; at most `size` bytes)
    let mut bytes = Vec::with_capacity(usize::try_from(loaded.size).unwrap_or(0));
    while (bytes.len() as u64) < loaded.size {
        let w = engine
            .host()
            .blob_read(
                &loaded.cid,
                bytes.len() as u64,
                lessdugin_core::host::BLOB_READ_MAX_BYTES,
            )
            .await
            .map_err(|e| EngineError::internal(format!("reading the image: {e}")))?;
        if w.is_empty() {
            break;
        }
        bytes.extend_from_slice(&w);
    }
    bytes.truncate(usize::try_from(loaded.size).unwrap_or(usize::MAX));
    Ok(crate::image::MediaBytes(std::sync::Arc::new(bytes)))
}

/// A setting's value as the `:` line types it: a JSON list, object or quoted string as
/// written (`mediaAutoloadHosts ["cdn.example"]`), anything else as the text it is
/// (`mediaAutoload off`). `set_setting` checks it against the setting's type.
pub fn setting_value(text: &str) -> serde_json::Value {
    if text.starts_with(['[', '{', '"'])
        && let Ok(v) = serde_json::from_str(text)
    {
        return v;
    }
    serde_json::Value::String(text.to_owned())
}

fn cadence_name(c: KarmaCadence) -> &'static str {
    match c {
        KarmaCadence::Realtime => "realtime",
        KarmaCadence::Daily => "daily",
        KarmaCadence::Weekly => "weekly",
    }
}

/// The cached record of `uri` as JSON.
async fn cached<H: Host>(engine: &Engine<H>, uri: &str) -> Result<serde_json::Value, EngineError> {
    let u = AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
    let row = RecordStore::new(engine.host().cache())
        .get(&u)
        .await
        .map_err(|e| EngineError::internal(e.to_string()))?
        .ok_or_else(|| EngineError::not_found(format!("{uri} is not available")))?;
    row.value_json()
        .map_err(|e| EngineError::internal(format!("{uri}: {e}")))
}

/// The markdown of the cached body of `uri` (`""` for none). A body stored as a blob (spilled,
/// `lfm-1.md` §1.3) is its source as the engine fetched it for the thread's view: the thread is
/// read first, which fetches it when the host does not hold it yet.
async fn body_markdown<H: Host>(
    engine: &Engine<H>,
    uri: &str,
    v: &serde_json::Value,
) -> Result<String, EngineError> {
    let body = v.get("body");
    match body
        .and_then(|b| b.get("text"))
        .and_then(serde_json::Value::as_str)
    {
        Some(t) => return Ok(t.to_owned()),
        None if body.is_none() => return Ok(String::new()),
        None => {}
    }
    let blob = body
        .and_then(lessdugin_core::model::render::text_blob_cid)
        .ok_or_else(|| EngineError::invalid_input("this body is not one the TUI can edit"))?;
    if let Some(src) = stored_text_blob(engine, blob).await {
        return Ok(src);
    }
    engine.get_thread(uri, &ThreadOpts::default()).await?;
    stored_text_blob(engine, blob).await.ok_or_else(|| {
        EngineError::invalid_input(
            "this body is stored as a blob that could not be loaded; try again later",
        )
    })
}

/// `:edit`: your post or comment's draft as cached (the fields the CLI's `edit` keeps) and the
/// editor session on its text, with what the edit would suspend (`edit_preflight`, `social.md`
/// §4.5) for the user to confirm first.
async fn start_edit<H: Host>(
    engine: &Engine<H>,
    uri: &str,
    forum: &str,
) -> Result<(EditorRequest, Vec<ActionNotice>), EngineError> {
    let uri = item_uri(engine, uri, forum).await?;
    let me = engine.active_account().await?.map(|a| a.did);
    let u = AtUri::parse(&uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
    if me.as_deref() != Some(u.did().as_str()) {
        return Err(EngineError::invalid_input(
            "only your own posts and comments can be edited",
        ));
    }
    let v = cached(engine, &uri).await?;
    let markdown = body_markdown(engine, &uri, &v).await?;
    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let strong = |k: &str| {
        v.get(k)
            .and_then(|r| r.get("uri"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let (base, prefill, context, post) = match u.collection().as_str() {
        nsid::POST => {
            let kind = s("kind")
                .and_then(|k| serde_json::from_value::<PostKind>(serde_json::Value::String(k)).ok())
                .unwrap_or(PostKind::Article);
            // a quick take has no title (data-model.md PO4): its text is all body
            let quick = kind == PostKind::QuickTake;
            let title = s("title");
            let draft = PostDraft {
                forum: s("forum").unwrap_or_default(),
                kind,
                title: title.clone(),
                body: BodyDraft::text(markdown.clone()),
                link: strong("link").map(|uri| ExternalLinkDraft {
                    uri,
                    title: None,
                    description: None,
                }),
                tags: strs(v.get("tags")),
                labels: v
                    .get("labels")
                    .and_then(|l| l.get("values"))
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.get("val")?.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            let (prefill, context) = if quick {
                (markdown.clone(), "Editing your quick take".to_owned())
            } else {
                let title = title.unwrap_or_default();
                (
                    format!("{title}\n\n{markdown}"),
                    format!("Editing your post: {title}"),
                )
            };
            (
                Draft::Post {
                    id: None,
                    post: draft,
                },
                prefill,
                vec![context],
                !quick,
            )
        }
        nsid::COMMENT => (
            Draft::Comment {
                id: None,
                comment: CommentDraft {
                    root: strong("root").unwrap_or_default(),
                    parent: strong("parent"),
                    body: BodyDraft::text(markdown.clone()),
                    quote: v
                        .get("quote")
                        .and_then(|q| serde_json::from_value::<TextQuote>(q.clone()).ok()),
                },
            },
            markdown,
            vec!["Editing your comment".to_owned()],
            false,
        ),
        _ => {
            return Err(EngineError::invalid_ref(format!(
                "{uri} is not a post or a comment"
            )));
        }
    };
    let notices = engine.edit_preflight(&uri).await?;
    Ok((
        EditorRequest {
            compose: Compose::Edit {
                uri,
                base: Box::new(base),
            },
            template: crate::editor::edit_template(&prefill, &context, post),
        },
        notices,
    ))
}

/// The source of the forum's spilled `about` `md`: the `textBlob` the host holds, else as
/// `forum_open` fetches it (it waits for the owner's PDS as a thread does for a body).
async fn spilled_about<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    md: &serde_json::Value,
) -> Result<String, EngineError> {
    let blob = lessdugin_core::model::render::text_blob_cid(md).ok_or_else(|| {
        EngineError::invalid_input("the forum's about text is not one the TUI can edit")
    })?;
    if let Some(src) = stored_text_blob(engine, blob).await {
        return Ok(src);
    }
    engine.forum_open(forum).await?;
    stored_text_blob(engine, blob).await.ok_or_else(|| {
        EngineError::invalid_input(
            "the forum's about text is stored as a blob that could not be loaded; try again later",
        )
    })
}

fn strs(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `:forum edit`: the forum's cached declaration as a draft, with `changes` applied
/// (`forum_update` writes every field it carries).
async fn forum_draft<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    changes: &ForumChanges,
) -> Result<ForumDraft, EngineError> {
    let v = cached(engine, forum).await?;
    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let about = match v.get("about") {
        None => None,
        Some(b) => match b.get("text").and_then(serde_json::Value::as_str) {
            Some(t) => Some(BodyDraft::text(t)),
            // a spilled `about` (`data-model.md` §5.3) is sent as its source, which the outbox
            // spills again: as `forum_open` fetched it from the owner's PDS for the header
            None => Some(BodyDraft::text(spilled_about(engine, forum, b).await?)),
        },
    };
    let empty_is_none = |o: &Option<String>| o.clone().filter(|t| !t.is_empty());
    Ok(ForumDraft {
        // the slug is fixed; empty keeps it
        slug: String::new(),
        name: changes
            .name
            .clone()
            .or_else(|| s("name"))
            .unwrap_or_default(),
        description: match &changes.description {
            Some(_) => empty_is_none(&changes.description),
            None => s("description"),
        },
        about,
        membership: changes
            .membership
            .unwrap_or(match s("membership").as_deref() {
                Some("open") => MembershipMode::Open,
                _ => MembershipMode::Approval,
            }),
        default_tier: changes
            .default_tier
            .unwrap_or(match s("defaultTier").as_deref() {
                Some("frontpage") => DefaultTier::Frontpage,
                _ => DefaultTier::Personal,
            }),
        featured_tags: strs(v.get("featuredTags")),
        // an empty list keeps the moderators (they change through mods_set)
        moderators: Vec::new(),
        report_contact: match &changes.report_contact {
            Some(_) => empty_is_none(&changes.report_contact),
            None => s("reportContact"),
        },
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
/// uri, and a `#restrict` assertion that adds capabilities completed with the ones the
/// account's current record restricts (the engine keeps the dimension an assertion leaves out,
/// `engine-api.md` §3.5).
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
        Some(RestrictDim::AddCapabilities),
        ModActionIntent::Assert {
            forum,
            scope,
            subject,
            action:
                ModActionValue::Restrict {
                    capabilities: Some(capabilities),
                    ..
                },
            ..
        },
    ) = (merge, &mut intent)
    else {
        return Ok(intent);
    };
    let held = held_capabilities(engine, forum, *scope, subject).await?;
    let added = std::mem::take(capabilities);
    *capabilities = ALL_CAPABILITIES
        .iter()
        .filter(|c| held.contains(c) || added.contains(c))
        .cloned()
        .collect();
    Ok(intent)
}

/// The capabilities the account's own `#restrict` record on `subject` in `forum` restricts (one
/// record per scope, subject and key, `data-model.md` §6.7); none when it has no record, or its
/// `until` has passed. A write of that record still in the outbox is newer than the cache,
/// which holds confirmed writes only (`lessdugin_core::outbox::apply`): the latest such write
/// is the state, so two restrictions in quick succession, or made offline, keep each other.
async fn held_capabilities<H: Host>(
    engine: &Engine<H>,
    forum: &str,
    scope: ModScope,
    subject: &str,
) -> Result<Vec<Capability>, EngineError> {
    let Some(me) = engine.active_account().await?.map(|a| a.did) else {
        return Ok(Vec::new());
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
        return Ok(Vec::new());
    };
    let pending = engine.outbox().pending(&me).await?;
    let latest = pending
        .iter()
        .rev()
        .flat_map(|w| w.ops.iter().rev())
        .find(|op| op.collection == nsid::MOD_ACTION && op.rkey == rkey);
    let value = match latest {
        // a retraction on its way: nothing is held
        Some(op) if op.action == OpAction::Delete => return Ok(Vec::new()),
        Some(op) => op
            .value
            .as_deref()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(v).ok()),
        None => match RecordStore::new(engine.host().cache()).get(&uri).await {
            Ok(Some(row)) => row.value_json().ok(),
            _ => None,
        },
    };
    let Some(action) = value.as_ref().and_then(|v| v.get("action")) else {
        return Ok(Vec::new());
    };
    // a restriction whose `until` passed holds nothing, as the engine reads it (`social.md` §4.2)
    let until = value
        .as_ref()
        .and_then(|v| v.get("until"))
        .and_then(serde_json::Value::as_str)
        .and_then(|u| lessdugin_atp::syntax::Datetime::parse(u).ok());
    if until.is_some_and(|u| u.timestamp_micros() <= engine.host().now_ms().saturating_mul(1000)) {
        return Ok(Vec::new());
    }
    Ok(action
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| serde_json::from_value(c.clone()).ok())
                .collect()
        })
        .unwrap_or_default())
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
/// `cid` (for a spilled body, its fetched source), else the view's text.
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
    let spilled = match record.as_ref().and_then(|r| r.get("body")) {
        Some(md) => spilled_source(engine, md, view).await,
        None => None,
    };
    Body::from_source(record.as_ref(), spilled.as_deref(), view)
}

/// The source of the spilled body `md` (its `textBlob`, `lfm-1.md` §1.3) when the view was
/// rendered from it: the blob the engine fetched for the view (from the author's PDS, only for
/// items the fold lists) and the host stored CID-checked, read back here and decoded as the
/// engine decodes it (`data-model.md` §5.2 rule 3). Nothing is fetched here. `None` for an
/// inline body, and while the view shows it pending, missing or unreadable (its notice says
/// so, over the view's text).
async fn spilled_source<H: Host>(
    engine: &Engine<H>,
    md: &serde_json::Value,
    view: &lessdugin_core::dto::RenderedBody,
) -> Option<String> {
    use lessdugin_core::model::render::{
        NOTICE_MISSING_BODY, NOTICE_SPILLED, NOTICE_UNVERIFIED_BODY, text_blob_cid,
    };
    if md.get("text").is_some() {
        return None;
    }
    let blob = text_blob_cid(md)?;
    if view.notices.iter().any(|n| {
        [NOTICE_SPILLED, NOTICE_MISSING_BODY, NOTICE_UNVERIFIED_BODY].contains(&n.as_str())
    }) {
        return None;
    }
    stored_text_blob(engine, blob).await
}

/// The source of the `textBlob` `blob` the host holds (it stored it CID-checked when the engine
/// fetched it), at most `BLOB_TEXT_MAX_BYTES`, decoded as the engine decodes it (UTF-8, one
/// leading U+FEFF stripped); `None` when the host does not hold it or it is not a body.
async fn stored_text_blob<H: Host>(engine: &Engine<H>, blob: &str) -> Option<String> {
    let h = engine.host().blob_open(blob).await.ok()??;
    let max = lessdugin_core::limits::BLOB_TEXT_MAX_BYTES;
    if h.cid != blob || h.size > max {
        return None;
    }
    let mut bytes = Vec::new();
    while (bytes.len() as u64) < h.size {
        let w = engine
            .host()
            .blob_read(
                blob,
                bytes.len() as u64,
                lessdugin_core::host::BLOB_READ_MAX_BYTES,
            )
            .await
            .ok()?;
        if w.is_empty() {
            break;
        }
        bytes.extend_from_slice(&w);
    }
    bytes.truncate(usize::try_from(h.size).unwrap_or(usize::MAX));
    lessdugin_markdown::decode_text_blob(&bytes).ok()
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
