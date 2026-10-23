// SPDX-License-Identifier: AGPL-3.0-or-later
//! The operation dispatcher behind `WasmEngine.call(op, argsJson)`: one arm per engine
//! operation, named as in `packages/engine/src/api.ts` (`EngineApi`), with the arguments as a
//! JSON array in the method's parameter order (`undefined` becomes `null`) and the result as
//! dto/1 JSON (`"null"` for operations without one).
//!
//! It is generic over the host, so the native tests run it over `MemHost`. Operations the engine
//! does not implement yet ([`UNSUPPORTED`]) and unknown names fail with `unsupported`.

use lessdugin_core::dto::{
    CommentDraft, Draft, EditOpts, ForumDraft, ListPostsQuery, MembersQuery, PostDraft, RefContext,
    SyncOpts, ThreadOpts, Topic,
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
    "join",
    "leave",
    "members",
    "grant",
    "revoke",
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
    "outboxWake",
    "backgroundVerify",
    // §3.4
    "listPosts",
    "recentDiscussion",
    "getPost",
    "getThread",
    "history",
    "raw",
    // §3.5
    "publish",
    "comment",
    "edit",
    "delete",
    "retract",
    "undo",
    "retryWrite",
    "writeStatus",
    "writes",
    // §3.7
    "render",
    "safeHref",
    "verify",
    // host glue
    "externalChange",
];

/// `EngineApi` methods the engine does not implement yet: they reject with `unsupported`
/// (`engine-api.md` §3.9 and the later milestones).
pub const UNSUPPORTED: &[&str] = &[
    "forumDiscover",
    "forumUpdate",
    "modsSet",
    "tags",
    "tag",
    "sequences",
    "sequence",
    "user",
    "search",
    "notifications",
    "modQueue",
    "modLog",
    "drafts",
    "bookmarks",
    "subscriptions",
    "opinion",
    "modAction",
    "modAdopt",
    "wikiEdit",
    "seqCreate",
    "seqEdit",
    "seqAdd",
    "seqRemove",
    "seqMove",
    "redactRevision",
    "prepareMedia",
    "markRead",
    "saveDraft",
    "deleteDraft",
    "bookmark",
    "setSubscription",
    "mute",
    "settings",
    "setSetting",
    "exportLocal",
    "importLocal",
    "validateRecord",
    "deriveRkey",
    "runVector",
];

/// The `unsupported` error of an operation this build does not implement.
pub fn unsupported(op: &str) -> EngineError {
    let message = if UNSUPPORTED.contains(&op) {
        format!("{op} is not implemented by this engine yet")
    } else {
        format!("unknown engine operation {op:?}")
    };
    EngineError::new(ErrorCode::Unsupported, message).with_retryable(false)
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
            }))
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
        "raw" => {
            let (uri,): (String,) = args(op, a)?;
            out(&engine.raw(&uri).await?)
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
                OPS.contains(&m.as_str()) || UNSUPPORTED.contains(&m.as_str()),
                "EngineApi.{m} is neither in OPS nor in UNSUPPORTED"
            );
        }
        for m in UNSUPPORTED {
            assert!(
                api.iter().any(|a| a == m),
                "UNSUPPORTED lists {m}, not in EngineApi"
            );
            assert!(!OPS.contains(m), "{m} is in OPS and UNSUPPORTED");
        }
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
        for op in UNSUPPORTED {
            let err = call_err(&e, op, "[]");
            assert_eq!(err.code, ErrorCode::Unsupported, "{op}");
            assert!(!err.retryable);
        }
        let err = call_err(&e, "noSuchOp", "[]");
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert!(err.message.contains("unknown"), "{}", err.message);
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
