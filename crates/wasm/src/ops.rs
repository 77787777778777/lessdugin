// SPDX-License-Identifier: AGPL-3.0-or-later
//! The operation dispatcher behind `WasmEngine.call(op, argsJson)`: one arm per engine
//! operation, named as in `packages/engine/src/api.ts` (`EngineApi`), with the arguments as a
//! JSON array in the method's parameter order (`undefined` becomes `null`) and the result as
//! dto/1 JSON (`"null"` for operations without one).
//!
//! It is generic over the host, so the native tests run it over `MemHost`. Operations the engine
//! does not implement yet ([`UNSUPPORTED`], each with the milestone that brings it) and unknown
//! names fail with `unsupported`.

use lessdugin_core::dto::{
    CommentDraft, Draft, EditOpts, ForumDraft, KarmaCadence, ListPostsQuery, MediaInput,
    MembersQuery, ModActionIntent, ModsChange, NotificationsQuery, OpinionIntent, PostDraft,
    PublicActionKind, RefContext, Subscription, SyncOpts, ThreadOpts, Topic,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineError, ErrorCode};
use lessdugin_core::host::Host;
use lessdugin_markdown::HrefCtx;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// The operations [`call`] implements: every implemented `EngineApi` method, plus the host
/// glue of the worker (`outboxWake`, `externalChange`) and the one-shot runs (`outboxFlush`,
/// `liveStep`).
pub const OPS: &[&str] = &[
    // §3.1
    "resolveActor",
    "accounts",
    "addAccount",
    "removeAccount",
    "setActiveAccount",
    "activeAccount",
    "parseRef",
    // §3.2
    "forumOpen",
    "forumDiscover",
    "forumFollow",
    "forumUnfollow",
    "followed",
    "forumCreate",
    "forumUpdate",
    "join",
    "leave",
    "members",
    "grant",
    "revoke",
    "approve",
    "reject",
    "rejectedApplicants",
    "modsSet",
    // §3.3
    "syncForum",
    "syncThread",
    "liveTail",
    "liveStop",
    "liveStep",
    "verifyHandles",
    "outboxPump",
    "outboxStop",
    "outboxFlush",
    "outboxExpedite",
    "outboxWake",
    "backgroundVerify",
    // §3.4
    "listPosts",
    "recentDiscussion",
    "getPost",
    "backlinks",
    "getThread",
    "history",
    "notifications",
    "unreadCounts",
    "modQueue",
    "modLog",
    "raw",
    "bodySource",
    "bookmarks",
    "bookmarked",
    "subscriptions",
    "mutes",
    // §3.5
    "publish",
    "comment",
    "edit",
    "editPreflight",
    "delete",
    "retract",
    "opinion",
    "modAction",
    "modAdopt",
    "undo",
    "retryWrite",
    "writeStatus",
    "writes",
    // §3.6
    "loadMedia",
    "redactRevision",
    "prepareMedia",
    "drafts",
    "saveDraft",
    "deleteDraft",
    "settings",
    "setSetting",
    "exportLocal",
    "importLocal",
    "markRead",
    "markNotificationsRead",
    "markAllNotificationsRead",
    "bookmark",
    "setSubscription",
    "mute",
    "acknowledgedPublicActions",
    "publicActionAcknowledged",
    "acknowledgePublicAction",
    "karmaNotifications",
    "setKarmaNotifications",
    // §3.7
    "render",
    "safeHref",
    "validateRecord",
    "deriveRkey",
    "verify",
    // host glue
    "externalChange",
];

/// `EngineApi` methods the engine does not implement yet, with the milestone that brings each
/// (`engine-api.md` §3.9): they reject with `unsupported` (`detail.milestone`). Implementing
/// one later is a compatible change of engine API v0.
pub const UNSUPPORTED: &[(&str, &str)] = &[
    // §3.4: tags, sequences, user pages, search
    ("tags", "M5"),
    ("tag", "M5"),
    ("sequences", "M5"),
    ("sequence", "M5"),
    ("user", "M5"),
    ("search", "M5"),
    // §3.5: the wiki and sequences (M5), media and redaction (M4)
    ("wikiEdit", "M5"),
    ("seqCreate", "M5"),
    ("seqEdit", "M5"),
    ("seqAdd", "M5"),
    ("seqRemove", "M5"),
    ("seqMove", "M5"),
    // test builds only (the social vectors run natively and in the wasm32 test harness)
    ("runVector", "tests"),
];

/// The milestone that brings `op`, if it is one of [`UNSUPPORTED`].
pub fn unsupported_until(op: &str) -> Option<&'static str> {
    UNSUPPORTED
        .iter()
        .find_map(|(name, milestone)| (*name == op).then_some(*milestone))
}

/// The `unsupported` error of an operation this build does not implement.
pub fn unsupported(op: &str) -> EngineError {
    match unsupported_until(op) {
        Some(milestone) => EngineError::new(
            ErrorCode::Unsupported,
            format!("{op} is not implemented by this engine yet ({milestone})"),
        )
        .with_detail("milestone", milestone),
        None => EngineError::new(
            ErrorCode::Unsupported,
            format!("unknown engine operation {op:?}"),
        ),
    }
    .with_retryable(false)
}

fn args<T: DeserializeOwned>(op: &str, json: &str) -> Result<T, EngineError> {
    let json = match json.trim() {
        "" => "[]",
        s => s,
    };
    serde_json::from_str(json)
        .map_err(|e| EngineError::invalid_input(format!("{op}: bad arguments: {e}")))
}

/// [`args`] for an operation whose trailing parameters are optional in `EngineApi`: an array
/// shorter than `n` is padded with `null`, so a caller may leave them out (as `OP_ARITY` does in
/// the client for the operations it lists).
fn padded<T: DeserializeOwned>(op: &str, json: &str, n: usize) -> Result<T, EngineError> {
    let mut v: Vec<Value> = args(op, json)?;
    while v.len() < n {
        v.push(Value::Null);
    }
    serde_json::from_value(Value::Array(v))
        .map_err(|e| EngineError::invalid_input(format!("{op}: bad arguments: {e}")))
}

/// An operation without arguments: `[]` (or nothing).
fn no_args(op: &str, json: &str) -> Result<(), EngineError> {
    let v: Vec<Value> = args(op, json)?;
    if v.is_empty() {
        Ok(())
    } else {
        Err(EngineError::invalid_input(format!(
            "{op}: takes no arguments, got {}",
            v.len()
        )))
    }
}

fn out<T: Serialize>(v: &T) -> Result<String, EngineError> {
    serde_json::to_string(v).map_err(|e| EngineError::internal(format!("serialising: {e}")))
}

const NULL: &str = "null";

/// `markdown`/`field` (`HrefContext` in `types.ts`).
fn href_ctx(s: &str) -> Result<HrefCtx, EngineError> {
    match s {
        "markdown" => Ok(HrefCtx::Markdown),
        "field" => Ok(HrefCtx::Field),
        _ => Err(EngineError::invalid_input(format!(
            "safeHref: context {s:?} is not \"markdown\" or \"field\""
        ))),
    }
}

/// `safe_href` as `types.ts` `Href`: the renderer tags its enum with `type`, every engine DTO
/// with `kind`.
fn safe_href(raw: &str, ctx: HrefCtx) -> Result<Value, EngineError> {
    let Some(h) = lessdugin_markdown::safe_href(raw, ctx) else {
        return Ok(Value::Null);
    };
    let v = serde_json::to_value(&h).map_err(|e| EngineError::internal(e.to_string()))?;
    let Value::Object(mut fields) = v else {
        return Err(EngineError::internal("safe_href: not an object"));
    };
    let mut tagged = serde_json::Map::new();
    if let Some(kind) = fields.remove("type") {
        tagged.insert("kind".into(), kind);
    }
    tagged.extend(fields);
    Ok(Value::Object(tagged))
}

/// `prepareMedia`'s engine API v0 argument, `MediaFile { bytes, mime?, name?, alt? }` (`mime` is
/// the declared type), as the bytes and the hints of the current form.
fn media_file(
    op: &str,
    mut file: serde_json::Map<String, Value>,
) -> Result<(Vec<u8>, Option<MediaInput>), EngineError> {
    let bad = |e: String| EngineError::invalid_input(format!("{op}: bad arguments: {e}"));
    let bytes = match file.remove("bytes") {
        // a Uint8Array that crossed a structured clone arrives as an index-keyed object
        Some(Value::Object(m)) => {
            let mut v: Vec<(usize, Value)> = m
                .into_iter()
                .map(|(k, b)| k.parse().map(|i| (i, b)).map_err(|_| bad("bytes".into())))
                .collect::<Result<_, _>>()?;
            v.sort_by_key(|(i, _)| *i);
            Value::Array(v.into_iter().map(|(_, b)| b).collect())
        }
        Some(b) => b,
        None => return Err(bad("a MediaFile without bytes".into())),
    };
    let bytes: Vec<u8> = serde_json::from_value(bytes).map_err(|e| bad(e.to_string()))?;
    if let Some(m) = file.remove("mime") {
        file.entry("declaredMime").or_insert(m);
    }
    let hints: MediaInput =
        serde_json::from_value(Value::Object(file)).map_err(|e| bad(e.to_string()))?;
    Ok((bytes, Some(hints)))
}

/// Runs `op` with `args` (a JSON array) on `engine`; the result as dto/1 JSON.
pub async fn call<H: Host>(engine: &Engine<H>, op: &str, a: &str) -> Result<String, EngineError> {
    match op {
        // §3.1 Identity, accounts, refs
        "resolveActor" => {
            let (input,): (String,) = args(op, a)?;
            out(&engine.resolve_actor(&input).await?)
        }
        "accounts" => {
            no_args(op, a)?;
            out(&engine.accounts().await?)
        }
        "addAccount" => {
            let (info,): (AccountInfo,) = args(op, a)?;
            out(&engine.add_account(info).await?)
        }
        "removeAccount" => {
            let (did, purge): (String, bool) = args(op, a)?;
            engine.remove_account(&did, purge).await?;
            Ok(NULL.into())
        }
        "setActiveAccount" => {
            let (did,): (String,) = args(op, a)?;
            engine.set_active_account(&did).await?;
            Ok(NULL.into())
        }
        "activeAccount" => {
            no_args(op, a)?;
            out(&engine.active_account().await?)
        }
        "parseRef" => {
            let (input, ctx): (String, Option<RefContext>) = args(op, a)?;
            out(&engine.parse_ref(&input, &ctx.unwrap_or_default()).await?)
        }

        // §3.2 Forums and membership
        "forumOpen" => {
            let (r,): (String,) = args(op, a)?;
            out(&engine.forum_open(&r).await?)
        }
        "forumDiscover" => {
            let (q,): (String,) = args(op, a)?;
            out(&engine.forum_discover(&q).await?)
        }
        "forumFollow" => {
            let (uri, alias): (String, Option<String>) = args(op, a)?;
            engine.forum_follow(&uri, alias.as_deref()).await?;
            Ok(NULL.into())
        }
        "forumUnfollow" => {
            let (uri,): (String,) = args(op, a)?;
            engine.forum_unfollow(&uri).await?;
            Ok(NULL.into())
        }
        "followed" => {
            no_args(op, a)?;
            out(&engine.followed().await?)
        }
        "forumCreate" => {
            let (draft,): (ForumDraft,) = args(op, a)?;
            out(&engine.forum_create(&draft).await?)
        }
        "forumUpdate" => {
            let (uri, draft): (String, ForumDraft) = args(op, a)?;
            out(&engine.forum_update(&uri, &draft).await?)
        }
        "join" => {
            let (uri, note): (String, Option<String>) = args(op, a)?;
            out(&engine.join(&uri, note.as_deref()).await?)
        }
        "leave" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.leave(&uri).await?)
        }
        "members" => {
            let (uri, q): (String, Option<MembersQuery>) = args(op, a)?;
            out(&engine.members(&uri, &q.unwrap_or_default()).await?)
        }
        // one ticket per actor
        "grant" => {
            let (uri, actors): (String, Vec<String>) = args(op, a)?;
            out(&engine.grant(&uri, &actors).await?)
        }
        "revoke" => {
            let (uri, actors): (String, Vec<String>) = args(op, a)?;
            out(&engine.revoke(&uri, &actors).await?)
        }
        "approve" => {
            let (uri, actors): (String, Vec<String>) = args(op, a)?;
            out(&engine.approve(&uri, &actors).await?)
        }
        "reject" => {
            let (uri, actors): (String, Vec<String>) = args(op, a)?;
            out(&engine.reject(&uri, &actors).await?)
        }
        "rejectedApplicants" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.rejected_applicants(&uri).await?)
        }
        "modsSet" => {
            let (uri, change): (String, ModsChange) = args(op, a)?;
            out(&engine.mods_set(&uri, &change).await?)
        }

        // §3.3 Sync
        "syncForum" => {
            let (uri, opts): (String, Option<SyncOpts>) = args(op, a)?;
            out(&engine.sync_forum(&uri, &opts.unwrap_or_default()).await?)
        }
        "syncThread" => {
            let (uri, opts): (String, Option<SyncOpts>) = args(op, a)?;
            out(&engine.sync_thread(&uri, &opts.unwrap_or_default()).await?)
        }
        "liveTail" => {
            no_args(op, a)?;
            engine.live_tail().await?;
            Ok(NULL.into())
        }
        "liveStop" => {
            no_args(op, a)?;
            engine.live_stop();
            Ok(NULL.into())
        }
        // `LiveRound` is not a DTO: a plain summary for tests and diagnostics
        "liveStep" => {
            no_args(op, a)?;
            let r = engine.live_step().await?;
            out(&json!({
                "polled": r.polled,
                "reconciles": r.reconciles,
                "synced": r.synced,
                "changed": r.changed,
                "verified": r.verified,
                "gc": r.gc,
                "nextMs": r.next_ms,
            }))
        }
        "verifyHandles" => {
            let (n,): (u32,) = args(op, a)?;
            out(&engine.verify_handles(n as usize).await)
        }
        "outboxPump" => {
            no_args(op, a)?;
            engine.outbox_pump().await?;
            Ok(NULL.into())
        }
        "outboxStop" => {
            no_args(op, a)?;
            engine.outbox_stop();
            Ok(NULL.into())
        }
        // `PumpReport` is not a DTO either
        "outboxFlush" => {
            no_args(op, a)?;
            let r = engine.outbox_flush().await?;
            out(&json!({
                "processed": r.processed,
                "done": r.done,
                "failed": r.failed,
                "conflicts": r.conflicts,
                "throttled": r.throttled,
                "needsLogin": r.needs_login,
            }))
        }
        "outboxExpedite" => {
            no_args(op, a)?;
            out(&engine.outbox_expedite().await?)
        }
        // another tab enqueued a write: wake this engine's pump
        "outboxWake" => {
            no_args(op, a)?;
            engine.outbox().wake_all();
            Ok(NULL.into())
        }
        "backgroundVerify" => {
            no_args(op, a)?;
            engine.background_verify().await?;
            Ok(NULL.into())
        }

        // §3.4 Queries
        "listPosts" => {
            let (q,): (ListPostsQuery,) = args(op, a)?;
            out(&engine.list_posts(&q).await?)
        }
        "recentDiscussion" => {
            let (forum, cursor, limit): (String, Option<String>, Option<u32>) = args(op, a)?;
            out(&engine
                .recent_discussion(&forum, cursor.as_deref(), limit.unwrap_or(0))
                .await?)
        }
        "getPost" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.get_post(&uri).await?)
        }
        "backlinks" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.backlinks(&uri).await?)
        }
        "getThread" => {
            let (uri, opts): (String, Option<ThreadOpts>) = args(op, a)?;
            out(&engine.get_thread(&uri, &opts.unwrap_or_default()).await?)
        }
        "history" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.history(&uri).await?)
        }
        "notifications" => {
            let (q,): (Option<NotificationsQuery>,) = args(op, a)?;
            out(&engine.notifications(&q.unwrap_or_default()).await?)
        }
        "unreadCounts" => {
            no_args(op, a)?;
            out(&engine.unread_counts().await?)
        }
        "modQueue" => {
            let (forum,): (String,) = args(op, a)?;
            out(&engine.mod_queue(&forum).await?)
        }
        "modLog" => {
            let (forum, cursor, limit): (String, Option<String>, Option<u32>) = args(op, a)?;
            out(&engine
                .mod_log(&forum, cursor.as_deref(), limit.unwrap_or(0))
                .await?)
        }
        "raw" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.raw(&uri).await?)
        }
        "bodySource" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.body_source(&uri).await?)
        }
        "bookmarks" => {
            no_args(op, a)?;
            out(&engine.bookmarks().await?)
        }
        "bookmarked" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.bookmarked(&uri).await?)
        }
        "subscriptions" => {
            no_args(op, a)?;
            out(&engine.subscriptions().await?)
        }
        "mutes" => {
            no_args(op, a)?;
            out(&engine.mutes().await?)
        }

        // §3.5 Writes
        "publish" => {
            let (draft,): (PostDraft,) = args(op, a)?;
            out(&engine.publish(&draft).await?)
        }
        "comment" => {
            let (draft,): (CommentDraft,) = args(op, a)?;
            out(&engine.comment(&draft).await?)
        }
        "edit" => {
            let (uri, draft, opts): (String, Draft, Option<EditOpts>) = args(op, a)?;
            out(&engine.edit(&uri, &draft, &opts.unwrap_or_default()).await?)
        }
        "editPreflight" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.edit_preflight(&uri).await?)
        }
        "delete" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.delete(&uri).await?)
        }
        "loadMedia" => {
            // `[cid, media?]`: the item's MediaView, or only its author's DID, names the author
            // whose PDS has the blob (sync.md §11.1); absent, the active account's own repo
            let v: Vec<Value> = args(op, a)?;
            let cid = v.first().and_then(Value::as_str).ok_or_else(|| {
                EngineError::invalid_input(format!("{op}: bad arguments: no cid"))
            })?;
            let view = match v.get(1) {
                Some(m @ Value::Object(_)) => Some(
                    serde_json::from_value::<lessdugin_core::dto::MediaView>(m.clone()).map_err(
                        |e| EngineError::invalid_input(format!("{op}: bad arguments: {e}")),
                    )?,
                ),
                _ => None,
            };
            let owner = match (v.get(1), &view) {
                (_, Some(m)) => Some(lessdugin_core::media::MediaOwner::of_view(m)),
                (Some(Value::String(did)), None) => {
                    Some(lessdugin_core::media::MediaOwner::author(did))
                }
                (None | Some(Value::Null), None) => None,
                (Some(_), None) => {
                    return Err(EngineError::invalid_input(format!(
                        "{op}: bad arguments: the second is a MediaView or a DID"
                    )));
                }
            };
            out(&engine.load_media(cid, owner).await?)
        }
        "drafts" => out(&engine.drafts().await?),
        "saveDraft" => {
            let (draft,): (Draft,) = args(op, a)?;
            out(&engine.save_draft(&draft).await?)
        }
        "deleteDraft" => {
            let (id,): (String,) = args(op, a)?;
            engine.delete_draft(&id).await?;
            out(&())
        }
        "settings" => out(&engine.settings().await?),
        "setSetting" => {
            // §3.6: a field of `Settings` and a value of its type; `null` returns it to its
            // default
            let (key, value): (String, Option<Value>) = padded(op, a, 2)?;
            engine.set_setting(&key, value.as_ref()).await?;
            Ok(NULL.into())
        }
        "exportLocal" => {
            // the client's declaration takes no scope, so the server default applies
            out(&engine
                .export_local(lessdugin_core::engine::ExportScope::default())
                .await?)
        }
        "importLocal" => {
            // `(export, scope?)`: `EngineApi.importLocal(json)` may leave the scope out
            let (json, scope): (lessdugin_core::engine::LocalExport, Option<String>) =
                padded(op, a, 2)?;
            // `ExportScope` is a plain enum with no serde form, so the client names it: "mine"
            // (the default, PRIV-8) or "everything"
            let scope = match scope.as_deref() {
                Some("everything") => lessdugin_core::engine::ExportScope::Everything,
                _ => lessdugin_core::engine::ExportScope::Mine,
            };
            out(&engine.import_local(&json, scope).await?)
        }
        "redactRevision" => {
            // Always one write (engine-api.md §3.5), the client's single WriteTicket (engine API
            // v0): it lists the item's revisions from the PDS when it is sent, so an item
            // without history is a write that deletes nothing.
            let (uri,): (String,) = args(op, a)?;
            let tickets = engine.redact_revision(&uri).await?;
            let t = tickets
                .into_iter()
                .next()
                .ok_or_else(|| EngineError::internal("redact_revision queued no write"))?;
            out(&t)
        }
        "prepareMedia" => {
            // The bytes are a separate argument from the hints (engine-api.md §3.6), so
            // this op takes them as a number array: the bridge serialises a Uint8Array
            // that way, and `prepare_media` is synchronous - it allocates nothing large and
            // touches no network, so it does not need the outbox.
            // Engine API v0's form, one `MediaFile { bytes, mime?, name?, alt? }`, is accepted
            // too.
            let (first, input): (Value, Option<MediaInput>) = padded(op, a, 2)?;
            let (bytes, hints) = match first {
                Value::Object(file) => media_file(op, file)?,
                bytes => (
                    serde_json::from_value::<Vec<u8>>(bytes).map_err(|e| {
                        EngineError::invalid_input(format!("{op}: bad arguments: {e}"))
                    })?,
                    input,
                ),
            };
            let hints = hints.unwrap_or(MediaInput {
                declared_mime: None,
                alt: None,
                name: None,
                aspect_ratio: None,
            });
            out(&engine.prepare_media(bytes, &hints).await?)
        }
        "retract" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.retract(&uri).await?)
        }
        "opinion" => {
            let (subject, intent): (String, OpinionIntent) = args(op, a)?;
            out(&engine.opinion(&subject, &intent).await?)
        }
        "modAction" => {
            let (intent,): (ModActionIntent,) = args(op, a)?;
            out(&engine.mod_action(&intent).await?)
        }
        "modAdopt" => {
            let (forum, former): (String, String) = args(op, a)?;
            out(&engine.mod_adopt(&forum, &former).await?)
        }
        "undo" => {
            let (id,): (String,) = args(op, a)?;
            out(&engine.undo(&id).await?)
        }
        "retryWrite" => {
            let (id,): (String,) = args(op, a)?;
            out(&engine.retry_write(&id).await?)
        }
        "writeStatus" => {
            let (id,): (String,) = args(op, a)?;
            out(&engine.write_status(&id).await?)
        }
        "writes" => {
            no_args(op, a)?;
            out(&engine.writes().await?)
        }

        // §3.6 Local state
        "markRead" => {
            let (uri,): (String,) = args(op, a)?;
            engine.mark_read(&uri).await?;
            Ok(NULL.into())
        }
        "markNotificationsRead" => {
            let (ids,): (Vec<String>,) = args(op, a)?;
            engine.mark_notifications_read(&ids).await?;
            Ok(NULL.into())
        }
        "markAllNotificationsRead" => {
            no_args(op, a)?;
            engine.mark_all_notifications_read().await?;
            Ok(NULL.into())
        }
        "bookmark" => {
            let (uri, on): (String, bool) = args(op, a)?;
            engine.bookmark(&uri, on).await?;
            Ok(NULL.into())
        }
        "setSubscription" => {
            let (sub, on): (Subscription, bool) = args(op, a)?;
            engine.set_subscription(&sub, on).await?;
            Ok(NULL.into())
        }
        "mute" => {
            let (actor, on): (String, bool) = args(op, a)?;
            engine.mute(&actor, on).await?;
            Ok(NULL.into())
        }
        "acknowledgedPublicActions" => {
            no_args(op, a)?;
            out(&engine.acknowledged_public_actions().await?)
        }
        "publicActionAcknowledged" => {
            let (kind,): (PublicActionKind,) = args(op, a)?;
            out(&engine.public_action_acknowledged(kind).await?)
        }
        "acknowledgePublicAction" => {
            let (kind,): (PublicActionKind,) = args(op, a)?;
            engine.acknowledge_public_action(kind).await?;
            Ok(NULL.into())
        }
        "karmaNotifications" => {
            no_args(op, a)?;
            out(&engine.karma_notifications().await?)
        }
        "setKarmaNotifications" => {
            let (cadence,): (KarmaCadence,) = args(op, a)?;
            engine.set_karma_notifications(cadence).await?;
            Ok(NULL.into())
        }

        // §3.7 Pure functions
        "render" => {
            // `[markdown, media, opts]`: media as `MediaRef`s or `defs#media` values, and the
            // body's `profile` in `opts` (absent: lfm/1)
            let (markdown, media, opts): (String, Option<Vec<Value>>, Option<Value>) = args(op, a)?;
            let profile = match opts.as_ref().and_then(|o| o.get("profile")) {
                None | Some(Value::Null) => None,
                Some(Value::String(p)) => Some(p.as_str()),
                Some(other) => {
                    return Err(EngineError::invalid_input(format!(
                        "{op}: opts.profile is not a string: {other}"
                    )));
                }
            };
            out(&lessdugin_core::engine::render_draft(
                &markdown,
                &media.unwrap_or_default(),
                profile,
            )?)
        }
        "safeHref" => {
            let (raw, ctx): (String, String) = args(op, a)?;
            out(&safe_href(&raw, href_ctx(&ctx)?)?)
        }
        "validateRecord" => {
            let (collection, json): (String, Value) = args(op, a)?;
            out(&engine.validate_record(&collection, &json).await?)
        }
        "deriveRkey" => {
            let (input,): (lessdugin_core::dto::RkeyInput,) = args(op, a)?;
            out(&engine.derive_rkey(&input)?)
        }
        "verify" => {
            let (uri, deep): (String, Option<bool>) = args(op, a)?;
            out(&engine.verify(&uri, deep.unwrap_or(false)).await?)
        }

        // host glue: another engine over the same stores emitted `Changed { topics }`
        "externalChange" => {
            let (topics,): (Vec<Topic>,) = args(op, a)?;
            engine.external_change(&topics);
            Ok(NULL.into())
        }

        _ => Err(unsupported(op)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use lessdugin_core::engine::EngineConfig;
    use lessdugin_testkit::MemHost;

    fn engine() -> Engine<MemHost> {
        Engine::new(MemHost::new(), EngineConfig::default())
    }

    fn call_ok(e: &Engine<MemHost>, op: &str, a: &str) -> String {
        block_on(call(e, op, a)).unwrap_or_else(|err| panic!("{op}: {err:?}"))
    }

    fn call_err(e: &Engine<MemHost>, op: &str, a: &str) -> EngineError {
        block_on(call(e, op, a)).expect_err(op)
    }

    /// The `EngineApi` method names of `packages/engine/src/api.ts` (`  name(` at the start of a
    /// member line), except `subscribe`, which the worker implements.
    fn api_methods() -> Vec<String> {
        let src = include_str!("../../../packages/engine/src/api.ts");
        let body = &src[src.find("export interface EngineApi").expect("EngineApi")..];
        let body = &body[..body.find("\n}\n").expect("end of EngineApi")];
        body.lines()
            .filter_map(|l| {
                let l = l.strip_prefix("  ")?;
                let name: String = l
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                let rest = &l[name.len()..];
                (!name.is_empty() && (rest.starts_with('(') || rest.starts_with('<')))
                    .then_some(name)
            })
            .filter(|n| n != "subscribe")
            .collect()
    }

    #[test]
    fn every_api_method_is_dispatched_or_unsupported() {
        let api = api_methods();
        assert!(api.len() > 60, "parsed {} EngineApi methods", api.len());
        for m in &api {
            assert!(
                OPS.contains(&m.as_str()) || unsupported_until(m).is_some(),
                "EngineApi.{m} is neither in OPS nor in UNSUPPORTED"
            );
        }
        for (m, milestone) in UNSUPPORTED {
            assert!(
                api.iter().any(|a| a == m),
                "UNSUPPORTED lists {m}, not in EngineApi"
            );
            assert!(!OPS.contains(m), "{m} is in OPS and UNSUPPORTED");
            assert!(
                ["M4", "M5", "tests"].contains(milestone),
                "{m}: milestone {milestone}"
            );
        }
        let mut sorted = OPS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), OPS.len(), "OPS lists an operation twice");
        let glue = ["outboxWake", "externalChange", "outboxFlush", "liveStep"];
        for m in OPS {
            assert!(
                api.iter().any(|a| a == m) || glue.contains(m),
                "OPS lists {m}, which EngineApi does not declare"
            );
        }
    }

    /// `(name, required, total)` parameter counts of every `EngineApi` method (the first
    /// declaration of an overloaded one), from `api.ts`.
    fn api_arities() -> Vec<(String, usize, usize)> {
        let src = include_str!("../../../packages/engine/src/api.ts");
        let body = &src[src.find("export interface EngineApi").expect("EngineApi")..];
        let body = &body[..body.find("\n}\n").expect("end of EngineApi")];
        let mut out: Vec<(String, usize, usize)> = Vec::new();
        for name in api_methods() {
            let at = body
                .find(&format!("\n  {name}("))
                .or_else(|| body.find(&format!("\n  {name}<")))
                .expect(&name);
            let rest = &body[at..];
            let open = rest.find('(').expect("(");
            // the parameter list, to the matching parenthesis
            let (mut depth, mut end) = (0i32, open);
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '(' | '<' | '{' | '[' => depth += 1,
                    ')' | '>' | '}' | ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let list = &rest[open + 1..end];
            let (mut params, mut depth, mut cur) = (Vec::new(), 0i32, String::new());
            for c in list.chars() {
                match c {
                    '(' | '<' | '{' | '[' => depth += 1,
                    ')' | '>' | '}' | ']' => depth -= 1,
                    ',' if depth == 0 => {
                        params.push(std::mem::take(&mut cur));
                        continue;
                    }
                    _ => {}
                }
                cur.push(c);
            }
            params.push(cur);
            let params: Vec<String> = params
                .into_iter()
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect();
            let optional = params
                .iter()
                .filter(|p| p.split(':').next().is_some_and(|n| n.ends_with('?')))
                .count();
            if !out.iter().any(|(n, _, _)| n == &name) {
                out.push((name, params.len() - optional, params.len()));
            }
        }
        out
    }

    /// `OP_ARITY` of `packages/engine/src/client.ts`: the operations the client pads to their
    /// full arity with `null`.
    fn client_padding() -> Vec<(String, usize)> {
        let src = include_str!("../../../packages/engine/src/client.ts");
        let body = &src[src.find("OP_ARITY").expect("OP_ARITY")..];
        let body = &body[body.find('{').expect("{") + 1..body.find("});").expect("});")];
        body.lines()
            .filter_map(|l| {
                let (k, v) = l.trim().split_once(':')?;
                Some((
                    k.trim().to_owned(),
                    v.trim().trim_end_matches(',').parse().ok()?,
                ))
            })
            .collect()
    }

    /// The wasm dispatcher accepts every arity `EngineApi` lets a client send: the required
    /// parameters only (or `OP_ARITY`'s padding), and all of them. Every argument is `null`, so
    /// an operation fails on a type, never on the length of its argument tuple (`importLocal(json)`
    /// once rejected every call that left the scope out).
    #[test]
    fn every_api_arity_is_accepted() {
        let e = engine();
        let pads = client_padding();
        assert!(pads.iter().any(|(n, _)| n == "parseRef"), "{pads:?}");
        // long-lived, or not dispatched here
        let skip = ["liveTail", "outboxPump", "backgroundVerify"];
        let arities = api_arities();
        assert!(arities.len() > 60, "{}", arities.len());
        for (op, required, total) in &arities {
            if skip.contains(&op.as_str()) || unsupported_until(op).is_some() || *total == 0 {
                continue;
            }
            let sent = pads
                .iter()
                .find(|(n, _)| n == op)
                .map_or(*required, |(_, n)| *n);
            for n in [sent, *total] {
                let nulls = format!("[{}]", vec!["null"; n].join(","));
                if let Err(err) = block_on(call(&e, op, &nulls)) {
                    assert!(
                        !err.message.contains("invalid length"),
                        "{op} with {n} of {total} arguments: {}",
                        err.message
                    );
                }
            }
        }
    }

    #[test]
    fn discovery_and_backlinks_validate_their_arguments() {
        let e = engine();
        for op in ["forumDiscover", "backlinks"] {
            for bad in ["[]", "[null]", "[1]", r#"["x", "extra"]"#] {
                let err = call_err(&e, op, bad);
                assert_eq!(err.code, ErrorCode::InvalidInput, "{op} {bad}: {err}");
                assert!(err.message.contains("bad arguments"), "{op}: {err}");
            }
        }
    }

    #[test]
    fn unsupported_and_unknown_operations() {
        let e = engine();
        for (op, milestone) in UNSUPPORTED {
            let err = call_err(&e, op, "[]");
            assert_eq!(err.code, ErrorCode::Unsupported, "{op}");
            assert!(!err.retryable);
            assert_eq!(err.detail, Some(json!({ "milestone": milestone })), "{op}");
        }
        let err = call_err(&e, "noSuchOp", "[]");
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert!(err.message.contains("unknown"), "{}", err.message);
        assert_eq!(err.detail, None);
    }

    #[test]
    fn local_operations_round_trip_as_dto_json() {
        let e = engine();
        assert_eq!(call_ok(&e, "accounts", "[]"), "[]");
        assert_eq!(call_ok(&e, "accounts", ""), "[]");
        assert_eq!(call_ok(&e, "activeAccount", "[]"), "null");
        assert_eq!(call_ok(&e, "writes", "[]"), "[]");
        assert_eq!(call_ok(&e, "writeStatus", r#"["did:plc:x/1"]"#), "null");
        assert_eq!(call_ok(&e, "outboxWake", "[]"), "null");
        assert_eq!(call_ok(&e, "outboxStop", "[]"), "null");
        assert_eq!(call_ok(&e, "liveStop", "[]"), "null");
        assert_eq!(
            call_ok(
                &e,
                "externalChange",
                r#"[[{"kind":"forum","uri":"at://did:plc:abc/x.y.forum/f"},{"kind":"outbox"}]]"#
            ),
            "null"
        );
        // the parse of a SPA route needs no network
        let r = call_ok(
            &e,
            "parseRef",
            r##"["#/u/did:plc:ewvi7nxzyoun6zhxrhs64oiz", null]"##,
        );
        assert_eq!(
            r,
            r#"{"kind":"actor","did":"did:plc:ewvi7nxzyoun6zhxrhs64oiz"}"#
        );
    }

    #[test]
    fn pure_functions() {
        let e = engine();
        let body: Value =
            serde_json::from_str(&call_ok(&e, "render", r#"["Hello *world*", [], null]"#)).unwrap();
        assert!(
            body["html"].as_str().unwrap().contains("<em>world</em>"),
            "{body}"
        );
        assert_eq!(body["wordCount"], 2);
        // media (m4-20): a defs#media value as a record stores it, or a MediaRef from
        // prepareMedia, which renders as the value publish writes
        let cid = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku";
        let text = format!("![shown](cid:{cid})\n");
        let stored = json!([{"blob": {"$type": "blob", "ref": {"$link": cid},
            "mimeType": "image/png", "size": 1234}, "alt": "media alt",
            "aspectRatio": {"width": 800, "height": 600}}]);
        let prepared = json!([{"id": "m", "cid": cid, "mime": "image/png", "sizeBytes": 1234,
            "alt": "media alt", "aspectRatio": {"width": 800, "height": 600}}]);
        let html = format!(
            "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\"><img data-cid=\"{cid}\" \
             alt=\"shown\" width=\"800\" height=\"600\"></p>\n</div>\n"
        );
        for media in [&stored, &prepared] {
            let body: Value = serde_json::from_str(&call_ok(
                &e,
                "render",
                &json!([text, media, null]).to_string(),
            ))
            .unwrap();
            assert_eq!(body["html"], html, "{media}");
            assert_eq!(body["media"][0]["cid"], cid, "{body}");
            assert_eq!(body["media"][0]["alt"], "media alt", "{body}");
        }
        let err = call_err(&e, "render", r#"["x", [{"id":"m"}], null]"#);
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
        // the body's profile, in the options
        let body: Value = serde_json::from_str(&call_ok(
            &e,
            "render",
            r#"["text\n", [], {"html": true, "text": true, "styled": false, "profile": "lfm/2"}]"#,
        ))
        .unwrap();
        assert_eq!(body["notices"].as_array().unwrap().len(), 1, "{body}");
        let err = call_err(&e, "render", r#"["x", [], {"profile": 2}]"#);
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");

        let h: Value = serde_json::from_str(&call_ok(
            &e,
            "safeHref",
            r#"["https://Example.com/a", "field"]"#,
        ))
        .unwrap();
        assert_eq!(h["kind"], "external");
        assert_eq!(h["host"], "example.com");
        assert!(h.get("type").is_none());
        // the first key is the tag, like every engine enum
        assert!(
            call_ok(&e, "safeHref", r#"["https://example.com/", "field"]"#)
                .starts_with(r#"{"kind":"external""#)
        );
        assert_eq!(
            call_ok(&e, "safeHref", r#"["javascript:alert(1)", "markdown"]"#),
            "null"
        );
        assert_eq!(
            call_err(&e, "safeHref", r#"["https://x.y/", "html"]"#).code,
            ErrorCode::InvalidInput
        );
    }

    /// m4-20, acc-m4-6: the render vectors with media or a profile (which `render` refused, so
    /// the SPA's sanitiser check could not render them) render through the op to their
    /// expected html, with their `defs#media` values as they are.
    #[test]
    fn render_takes_the_media_and_profile_vectors() {
        let e = engine();
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/vectors");
        let mut checked = 0;
        for dir in ["lfm1", "xss"] {
            let mut files: Vec<_> = std::fs::read_dir(format!("{root}/{dir}"))
                .unwrap()
                .map(|f| f.unwrap().path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            files.sort();
            for path in files {
                let file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                for v in file["vectors"].as_array().into_iter().flatten() {
                    let input = &v["input"];
                    let (Some(text), Some(html)) =
                        (input["text"].as_str(), v["expected"]["html"].as_str())
                    else {
                        continue;
                    };
                    let media = input["media"].as_array().filter(|m| !m.is_empty());
                    let profile = input["profile"].as_str();
                    if media.is_none() && profile.is_none() {
                        continue;
                    }
                    let opts = json!({"html": true, "text": true, "styled": false,
                        "profile": profile});
                    let args = json!([text, media.cloned().unwrap_or_default(), opts]);
                    let body: Value =
                        serde_json::from_str(&call_ok(&e, "render", &args.to_string())).unwrap();
                    assert_eq!(
                        body["html"].as_str(),
                        Some(html),
                        "{}#{}",
                        path.display(),
                        v["name"]
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 40, "{checked} vectors");
    }

    #[test]
    fn bad_arguments_are_invalid_input() {
        let e = engine();
        for (op, a) in [
            ("accounts", "[1]"),
            ("getThread", "{}"),
            ("removeAccount", r#"["did:plc:x"]"#),
            ("listPosts", r#"[{"forum":"at://x"}]"#),
            ("resolveActor", "not json"),
        ] {
            let err = call_err(&e, op, a);
            assert_eq!(err.code, ErrorCode::InvalidInput, "{op} {a}: {err:?}");
        }
    }

    /// The M4 local-state operations keep `EngineApi`'s shapes (engine API v0): `setSetting` takes
    /// a field of `Settings` with a typed value, `importLocal` its export alone and reports what
    /// it wrote, `prepareMedia` the v0 `MediaFile` too.
    #[test]
    fn local_state_operations_take_the_declared_shapes() {
        let e = engine();
        assert_eq!(
            call_ok(
                &e,
                "setSetting",
                r#"["mediaAutoloadHosts", ["cdn.example"]]"#
            ),
            "null"
        );
        call_ok(&e, "setSetting", r#"["karmaNotifications", "weekly"]"#);
        let s: Value = serde_json::from_str(&call_ok(&e, "settings", "[]")).unwrap();
        assert_eq!(s["mediaAutoloadHosts"], json!(["cdn.example"]));
        assert_eq!(s["karmaNotifications"], "weekly");
        let err = call_err(&e, "setSetting", r#"["noSuchSetting", 1]"#);
        assert_eq!(err.code, ErrorCode::InvalidInput);

        let x = call_ok(&e, "exportLocal", "[]");
        let r: Value =
            serde_json::from_str(&call_ok(&e, "importLocal", &format!("[{x}]"))).unwrap();
        assert!(r["imported"].as_u64().unwrap() > 0, "{r}");
        assert_eq!(r["outOfScope"], 0, "{r}");

        // a 1x1 PNG, as engine API v0's `prepareMedia({ bytes, mime })`
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52,
            0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0d, 0x49,
            0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 5, 0, 1, 0xff,
            0x89, 0x99, 0x3d, 0x1d, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let bytes = serde_json::to_string(&png).unwrap();
        let v0 = call_ok(
            &e,
            "prepareMedia",
            &format!(r#"[{{"bytes": {bytes}, "mime": "image/png", "alt": "a dot"}}]"#),
        );
        let now = call_ok(
            &e,
            "prepareMedia",
            &format!(r#"[{bytes}, {{"declaredMime": "image/png", "alt": "a dot"}}]"#),
        );
        let (v0, now): (Value, Value) = (
            serde_json::from_str(&v0).unwrap(),
            serde_json::from_str(&now).unwrap(),
        );
        assert_eq!(
            (&v0["cid"], &v0["mime"], &v0["alt"]),
            (&now["cid"], &now["mime"], &now["alt"])
        );
    }

    #[test]
    fn validate_record_and_derive_rkey() {
        let e = engine();
        let forum =
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.temp.forum/meta";
        let post = json!({
            "$type": "online.7778777.lessdugin.temp.post",
            "forum": forum,
            "kind": "article",
            "title": "Hello",
            "body": {"$type": "online.7778777.lessdugin.temp.markdown", "text": "Hi."},
            "createdAt": "2024-01-01T00:00:00.000Z",
        });
        let r: Value = serde_json::from_str(&call_ok(
            &e,
            "validateRecord",
            &format!(r#"["post", {post}]"#),
        ))
        .unwrap();
        assert_eq!(r, json!({"valid": true, "errors": []}));
        let mut untitled = post.clone();
        untitled.as_object_mut().unwrap().remove("title");
        let r: Value = serde_json::from_str(&call_ok(
            &e,
            "validateRecord",
            &format!(r#"["online.7778777.lessdugin.temp.post", {untitled}]"#),
        ))
        .unwrap();
        assert_eq!(r["valid"], false, "{r}");
        assert_eq!(r["errors"], json!(["PO3"]), "{r}");

        let k = |input: Value| -> String {
            serde_json::from_str(&call_ok(&e, "deriveRkey", &format!("[{input}]"))).unwrap()
        };
        let fk = k(json!({"kind": "membership", "forum": forum}));
        let p = k(json!({"kind": "post", "forum": forum, "tid": "3l2s5xxv2ze2c"}));
        assert_eq!(p, format!("{fk}-3l2s5xxv2ze2c"));
        let root =
            format!("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.temp.post/{p}");
        let c = k(json!({"kind": "comment", "root": root, "tid": "3l2s6aaaaaaa2"}));
        assert_eq!(c, format!("{fk}-3l2s5xxv2ze2c-3l2s6aaaaaaa2"));
        let o = k(json!({"kind": "opinion", "forum": forum, "subject": root}));
        assert!(o.starts_with(&format!("{p}-")), "{o}");
        let m = k(
            json!({"kind": "modAction", "forum": forum, "scope": "forum",
            "subject": root, "actionKey": "hide"}),
        );
        assert!(
            m.starts_with(&format!("{fk}-")) && m.len() == fk.len() + 17,
            "{m}"
        );
        let err = call_err(
            &e,
            "deriveRkey",
            r#"[{"kind": "post", "forum": "x", "tid": "y"}]"#,
        );
        assert_eq!(err.code, ErrorCode::InvalidInput);
        // a comment is not a root (data-model.md §3: the root is a post keyed FK-TID)
        let on_comment = format!(
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.temp.comment/{c}"
        );
        let err = call_err(
            &e,
            "deriveRkey",
            &json!([{"kind": "comment", "root": on_comment, "tid": "3l2s6aaaaaaa3"}]).to_string(),
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
        // the actionKey of #pin and of a foreign action; none is the full $type of a known def
        let mk = |key: &str| {
            json!([{"kind": "modAction", "forum": forum, "scope": "forum",
                "subject": root, "actionKey": key}])
            .to_string()
        };
        for key in ["pin:forum", "com.example.action#x"] {
            assert!(block_on(call(&e, "deriveRkey", &mk(key))).is_ok(), "{key}");
        }
        let err = call_err(
            &e,
            "deriveRkey",
            &mk("online.7778777.lessdugin.temp.modAction#hide"),
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
        // the same key as the lexicon's derivation from the action itself
        let action = serde_json::Map::from_iter([(
            "$type".to_owned(),
            json!("online.7778777.lessdugin.temp.modAction#hide"),
        )]);
        let d = lessdugin_lexicon::keys::derive(
            lessdugin_lexicon::ns::NS,
            &lessdugin_lexicon::keys::RkeyInput::ModAction {
                forum,
                scope: None,
                subject: &root,
                action: &action,
            },
        )
        .unwrap();
        assert_eq!(m, d.rkey);
    }

    #[test]
    fn body_source_is_dispatched() {
        let e = engine();
        // the collection is checked before anything is asked of the network
        let err = call_err(
            &e,
            "bodySource",
            r#"["at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.y.membership/f"]"#,
        );
        assert_eq!(err.code, ErrorCode::InvalidRef, "{err:?}");
        let err = call_err(&e, "bodySource", "[]");
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
    }

    #[test]
    fn writes_need_an_account() {
        let e = engine();
        let err = call_err(
            &e,
            "join",
            r#"["at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.y.forum/f", null]"#,
        );
        assert_eq!(err.code, ErrorCode::AuthRequired, "{err:?}");
    }
}
