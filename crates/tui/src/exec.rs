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
    ActorView, BodyDraft, Capability, CommentDraft, DefaultTier, Draft, EditOpts,
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
        Effect::FetchMedia { cid, did, pds } => Action::MediaFetched {
            result: fetch_media(engine, &cid, &did, &pds).await,
            cid,
        },
        Effect::Editor(_) | Effect::DecodeMedia { .. } | Effect::Quit => return None,
    })
}

/// The bytes of the image blob `cid` in `did`'s repo at `pds`, for the decoder: fetched with
/// `Host::fetch_blob` (streamed and CID-checked; a blob the host already holds is not fetched
/// again), at most the decoder's input cap. A native host keeps the verified blob in a file
/// (`sync.md` §11.2), read here; a host that keeps it elsewhere (the test host) is asked for it
/// again into memory, capped and CID-checked the same way.
pub async fn fetch_media<H: Host>(
    engine: &Engine<H>,
    cid: &str,
    did: &str,
    pds: &str,
) -> Result<crate::image::MediaBytes, EngineError> {
    use std::io::Read as _;
    let max = crate::image::limits::MAX_INPUT_BYTES as u64;
    let pds = url::Url::parse(pds)
        .map_err(|e| EngineError::invalid_input(format!("the author's PDS {pds:?}: {e}")))?;
    let repo = lessdugin_atp::syntax::Did::parse(did)
        .map_err(|e| EngineError::invalid_input(format!("{did:?}: {e}")))?;
    let h = engine
        .host()
        .fetch_blob(pds.clone(), repo, cid.to_owned(), max)
        .await
        .map_err(|e| EngineError::internal(format!("the image could not be fetched: {e}")))?;
    if h.cid != cid {
        return Err(EngineError::internal("the host returned another blob"));
    }
    // (a task of the runtime, not the UI thread, and the read is capped)
    let path = std::path::Path::new(&h.locator);
    if path.is_absolute()
        && let Ok(f) = std::fs::File::open(path)
    {
        let mut bytes = Vec::new();
        f.take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| EngineError::internal(format!("reading the image: {e}")))?;
        if bytes.len() as u64 > max {
            return Err(EngineError::invalid_input(
                "the image is over the 64 MiB cap",
            ));
        }
        return Ok(crate::image::MediaBytes(std::sync::Arc::new(bytes)));
    }
    let parsed = lessdugin_atp::cid::parse(cid)
        .map_err(|e| EngineError::invalid_input(format!("{cid:?}: {e}")))?;
    let qs = lessdugin_atp::xrpc::query_string(&[("did", did), ("cid", cid)]);
    let url = url::Url::parse(&format!(
        "{}/xrpc/com.atproto.sync.getBlob?{qs}",
        pds.as_str().trim_end_matches('/')
    ))
    .map_err(|e| EngineError::internal(e.to_string()))?;
    let resp = engine
        .host()
        .http(
            lessdugin_core::host::HttpRequest::get(lessdugin_core::host::Target::Anon(url))
                .with_max_bytes(max),
        )
        .await
        .map_err(|e| EngineError::internal(format!("the image could not be fetched: {e}")))?;
    if !(200..300).contains(&resp.status) {
        return Err(EngineError::internal(format!(
            "the image could not be fetched: HTTP {}",
            resp.status
        )));
    }
    lessdugin_atp::cid::verify(&parsed, &resp.body)
        .map_err(|_| EngineError::internal("the image does not match its CID"))?;
    Ok(crate::image::MediaBytes(std::sync::Arc::new(resp.body)))
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

/// The markdown of a cached body (`""` for none); a body stored as a blob is not edited here,
/// as in the CLI.
fn body_markdown(v: &serde_json::Value) -> Result<String, EngineError> {
    let body = v.get("body");
    match body
        .and_then(|b| b.get("text"))
        .and_then(serde_json::Value::as_str)
    {
        Some(t) => Ok(t.to_owned()),
        None if body.is_none() => Ok(String::new()),
        None => Err(EngineError::invalid_input(
            "this body is stored as a blob; edit it with `lessdugin edit`",
        )),
    }
}

/// `:edit`: your post or comment's draft as cached (the fields the CLI's `edit` keeps) and the
/// editor session on its text.
async fn start_edit<H: Host>(
    engine: &Engine<H>,
    uri: &str,
    forum: &str,
) -> Result<EditorRequest, EngineError> {
    let uri = item_uri(engine, uri, forum).await?;
    let me = engine.active_account().await?.map(|a| a.did);
    let u = AtUri::parse(&uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
    if me.as_deref() != Some(u.did().as_str()) {
        return Err(EngineError::invalid_input(
            "only your own posts and comments can be edited",
        ));
    }
    let v = cached(engine, &uri).await?;
    let markdown = body_markdown(&v)?;
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
    Ok(EditorRequest {
        compose: Compose::Edit {
            uri,
            base: Box::new(base),
        },
        template: crate::editor::edit_template(&prefill, &context, post),
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
            None => {
                return Err(EngineError::invalid_input(
                    "the forum's about text is stored as a blob; edit the forum in the web app",
                ));
            }
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
        RestrictDim::AddCapabilities => {
            *nullify_votes = held_nullify;
            let added = std::mem::take(capabilities);
            *capabilities = ALL_CAPABILITIES
                .iter()
                .filter(|c| held_caps.contains(c) || added.contains(c))
                .cloned()
                .collect();
        }
        RestrictDim::Nullify => *capabilities = held_caps,
    }
    Ok(intent)
}

/// The account's own `#restrict` record on `subject` in `forum` (one per scope, subject and
/// key, `data-model.md` §6.7) as `(capabilities, nullifyVotes)`; nothing when it has none. A
/// write of that record still in the outbox is newer than the cache, which holds confirmed
/// writes only (`lessdugin_core::outbox::apply`): the latest such write is the state, so two
/// restrictions in quick succession, or made offline, keep each other.
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
    let pending = engine.outbox().pending(&me).await?;
    let latest = pending
        .iter()
        .rev()
        .flat_map(|w| w.ops.iter().rev())
        .find(|op| op.collection == nsid::MOD_ACTION && op.rkey == rkey);
    let value = match latest {
        // a retraction on its way: nothing is held
        Some(op) if op.action == OpAction::Delete => return Ok((Vec::new(), false)),
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
