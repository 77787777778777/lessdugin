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
    CommentDraft, Draft, EditOpts, ForumDraft, KarmaCadence, ListPostsQuery, MembersQuery,
    ModActionIntent, ModsChange, NotificationsQuery, OpinionIntent, PostDraft, PublicActionKind,
    RefContext, Subscription, SyncOpts, ThreadOpts, Topic,
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
    "getThread",
    "history",
    "notifications",
    "unreadCounts",
    "modQueue",
    "modLog",
    "raw",
    "bookmarks",
    "bookmarked",
    "subscriptions",
    "mutes",
    // §3.5
    "publish",
    "comment",
    "edit",
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
    "verify",
    // host glue
    "externalChange",
];

/// `EngineApi` methods the engine does not implement yet, with the milestone that brings each
/// (`engine-api.md` §3.9): they reject with `unsupported` (`detail.milestone`). Implementing
/// one later is a compatible change of engine API v0.
pub const UNSUPPORTED: &[(&str, &str)] = &[
    // §3.2: the forum directory
    ("forumDiscover", "M5"),
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
    ("redactRevision", "M4"),
    ("prepareMedia", "M4"),
    // §3.6: the engine's draft store, settings and local export (M4, with the editor, the
    // media policy and `export`)
    ("drafts", "M4"),
    ("saveDraft", "M4"),
    ("deleteDraft", "M4"),
    ("settings", "M4"),
    ("setSetting", "M4"),
    ("exportLocal", "M4"),
    ("importLocal", "M4"),
    // §3.7
    ("validateRecord", "M4"),
    ("deriveRkey", "M4"),
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
        "delete" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.delete(&uri).await?)
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
            let (markdown, media, _opts): (String, Option<Vec<Value>>, Option<Value>) =
                args(op, a)?;
            if media.as_ref().is_some_and(|m| !m.is_empty()) {
                // MediaRef → defs#media arrives with the media pipeline (M4)
                return Err(EngineError::new(
                    ErrorCode::Unsupported,
                    "render: media is not supported yet",
                ));
            }
            out(&lessdugin_core::model::render::render_markdown(
                &markdown,
                &Value::Array(Vec::new()),
                None,
            ))
        }
        "safeHref" => {
            let (raw, ctx): (String, String) = args(op, a)?;
            out(&safe_href(&raw, href_ctx(&ctx)?)?)
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
        let err = call_err(&e, "render", r#"["x", [{"id":"m"}], null]"#);
        assert_eq!(err.code, ErrorCode::Unsupported);

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
