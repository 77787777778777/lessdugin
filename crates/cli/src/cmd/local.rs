// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local state (`engine-api.md` §3.6, synthesis §6.10): `bookmark add|rm|ls`,
//! `subscribe|unsubscribe post|forum|user|tag`, `subs` and `config get|set`. Nothing here
//! writes to a repository; subscriptions feed `notifications` (`social.md` §16).
//!
//! `config set` routes to the engine's `set_setting`, which checks the key and the value's
//! shape; a `sources` that `config.toml` states is an explicit local pin (decision O-9): the
//! stored setting is kept, and `config get` says which value is in effect and why.

use lessdugin_core::dto::{BookmarkEntry, DTO_VERSION, Settings, Subscription, SubscriptionEntry};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_core::host::Host;
use lessdugin_native::host::NativeHost;
use serde::Serialize;
use serde_json::Value;

use crate::args::{
    BookmarkAction, BookmarkArgs, ConfigAction, ConfigArgs, SubscribeArgs, SubscribeTarget,
};
use crate::cmd::refs;
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, TermText, cat, lit, name, tt};
use crate::render;

#[derive(Serialize)]
struct BookmarkList<'a> {
    dto: u32,
    bookmarks: &'a [BookmarkEntry],
}

#[derive(Serialize)]
struct SubscriptionList<'a> {
    dto: u32,
    subscriptions: &'a [SubscriptionEntry],
}

#[derive(Serialize)]
struct SettingsView<'a> {
    dto: u32,
    settings: &'a Settings,
    /// The settings `config.toml` pins (O-9): their stored value is not the one in effect.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pinned: Vec<&'static str>,
}

// ---------------------------------------------------------------------------------------------
// bookmarks

/// `lessdugin bookmark [add|rm <ref>|ls]`.
pub async fn bookmark(ctx: &Ctx, args: BookmarkArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    match args.action.unwrap_or(BookmarkAction::Ls) {
        BookmarkAction::Add { reference } => set_bookmark(ctx, &engine, &reference, true).await,
        BookmarkAction::Rm { reference } => set_bookmark(ctx, &engine, &reference, false).await,
        BookmarkAction::Ls => {
            let list = engine.bookmarks().await?;
            if ctx.out.is_json() {
                print_bookmarks(ctx, &list);
                return Ok(());
            }
            if list.is_empty() {
                ctx.out.line(&[(lit("No bookmarks."), Style::Dim)]);
            }
            let w = list.len().to_string().len();
            for (i, b) in list.iter().enumerate() {
                let title = match &b.post {
                    Some(p) => tt(p.title.as_deref().unwrap_or("(untitled)")),
                    None => tt(&b.uri),
                };
                let mut meta = Vec::new();
                if let Some(p) = &b.post {
                    meta.push(render::actor(&p.author));
                }
                meta.push(cat([lit("saved "), tt(&render::time(b.saved_at_ms))]));
                ctx.out.line(&[
                    (tt(&format!("{:>w$}", i + 1)), Style::Bold),
                    (lit("  "), Style::Plain),
                    (title, Style::Bold),
                    (lit("  "), Style::Plain),
                    (joined(meta), Style::Dim),
                ]);
                if b.post.is_some() {
                    ctx.out.line(&[
                        (tt(&" ".repeat(w + 2)), Style::Plain),
                        (tt(&b.uri), Style::Dim),
                    ]);
                }
            }
            Ok(())
        }
    }
}

async fn set_bookmark(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    reference: &str,
    on: bool,
) -> Result<(), CliError> {
    let (uri, is_comment) = refs::post_or_comment(ctx, engine, reference).await?;
    if is_comment {
        return Err(CliError::usage("bookmarks are of posts: pass the post"));
    }
    engine.bookmark(&uri, on).await?;
    if ctx.out.is_json() {
        print_bookmarks(ctx, &engine.bookmarks().await?);
    } else {
        ctx.out.line(&[
            (
                lit(if on {
                    "Bookmarked "
                } else {
                    "Removed the bookmark of "
                }),
                Style::Good,
            ),
            (tt(&uri), Style::Bold),
        ]);
    }
    Ok(())
}

fn print_bookmarks(ctx: &Ctx, list: &[BookmarkEntry]) {
    ctx.out.json(&BookmarkList {
        dto: DTO_VERSION,
        bookmarks: list,
    });
}

fn joined(parts: Vec<TermText>) -> TermText {
    let mut out = TermText::empty();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.append(tt(" · "));
        }
        out.append(p);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// subscriptions

/// What `subscribe post|forum|user|tag …` names, resolved (`engine-api.md` §3.6 `subscribe`).
async fn target(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    t: SubscribeTarget,
) -> Result<Subscription, CliError> {
    Ok(match t {
        SubscribeTarget::Post { reference } => {
            let (uri, is_comment) = refs::post_or_comment(ctx, engine, &reference).await?;
            if is_comment {
                return Err(CliError::usage(
                    "a subscription is to a post's new comments: pass the post",
                ));
            }
            Subscription::Post { uri }
        }
        SubscribeTarget::Forum { forum } => Subscription::Forum {
            uri: refs::forum_or_current(ctx, engine, forum.as_deref()).await?,
        },
        SubscribeTarget::User { actor } => Subscription::User {
            did: engine.resolve_actor(&actor).await?.did,
        },
        SubscribeTarget::Tag { slug, forum } => Subscription::Tag {
            forum: refs::forum_or_current(ctx, engine, forum.as_deref()).await?,
            slug,
        },
    })
}

/// A subscription, for humans: what it is of.
fn describe(s: &Subscription) -> TermText {
    match s {
        Subscription::Post { uri } => cat([lit("new comments on "), tt(uri)]),
        Subscription::Forum { uri } => cat([lit("new posts in "), tt(uri)]),
        Subscription::User { did } => cat([lit("new posts by "), tt(did)]),
        Subscription::Tag { forum, slug } => {
            cat([lit("new posts tagged "), name(slug), lit(" in "), tt(forum)])
        }
        Subscription::Sequence { uri } => cat([lit("new entries of "), tt(uri)]),
    }
}

/// `lessdugin subscribe|unsubscribe post <ref>|forum [forum]|user <actor>|tag <slug>`.
pub async fn subscribe(ctx: &Ctx, args: SubscribeArgs, on: bool) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let s = target(ctx, &engine, args.target).await?;
    engine.set_subscription(&s, on).await?;
    if ctx.out.is_json() {
        print_subscriptions(ctx, &engine.subscriptions().await?);
    } else {
        ctx.out.line(&[
            (
                lit(if on {
                    "Subscribed to "
                } else {
                    "Unsubscribed from "
                }),
                Style::Good,
            ),
            (describe(&s), Style::Plain),
        ]);
    }
    Ok(())
}

/// `lessdugin subs`.
pub async fn subs(ctx: &Ctx) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let list = engine.subscriptions().await?;
    if ctx.out.is_json() {
        print_subscriptions(ctx, &list);
        return Ok(());
    }
    if list.is_empty() {
        ctx.out.line(&[(lit("No subscriptions."), Style::Dim)]);
    }
    for e in &list {
        ctx.out.line(&[
            (describe(&e.subscription), Style::Plain),
            (
                cat([lit("  since "), tt(&render::time(e.since_ms))]),
                Style::Dim,
            ),
        ]);
    }
    Ok(())
}

fn print_subscriptions(ctx: &Ctx, list: &[SubscriptionEntry]) {
    ctx.out.json(&SubscriptionList {
        dto: DTO_VERSION,
        subscriptions: list,
    });
}

// ---------------------------------------------------------------------------------------------
// config

/// The settings that take a list, whose value on the command line may be comma separated.
const LIST_SETTINGS: [&str; 4] = [
    "indexers",
    "externalImagesForums",
    "mediaAutoloadHosts",
    "acknowledgedPublicActions",
];

/// A value of `config set <key> <value>` as `set_setting` takes it: JSON when it reads as a
/// list, an object or a quoted string; for a list setting a bare value is a comma-separated
/// list (`indexers`: of URLs); otherwise the string. `set_setting` checks it against the
/// setting's type.
fn setting_value(key: &str, text: &str, now_ms: i64) -> Value {
    if text.starts_with(['[', '{', '"'])
        && let Ok(v) = serde_json::from_str(text)
    {
        return v;
    }
    if LIST_SETTINGS.contains(&key) {
        let items = text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                if key == "indexers" {
                    serde_json::json!({ "url": s, "grantedAtMs": now_ms })
                } else {
                    Value::String(s.to_owned())
                }
            })
            .collect();
        return Value::Array(items);
    }
    Value::String(text.to_owned())
}

/// The settings pinned outside the stored setting (decision O-9): `sources`, when the file
/// states it or devnet mode fixes it to `direct`.
fn pinned(ctx: &Ctx) -> Vec<&'static str> {
    if crate::ctx::engine_config(&ctx.config)
        .sources_flag
        .is_some()
    {
        vec!["sources"]
    } else {
        Vec::new()
    }
}

/// A setting's value, for humans: a string bare, anything else as JSON.
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Prints the settings in effect (`key`: one of them).
async fn show(ctx: &Ctx, engine: &Engine<NativeHost>, key: Option<&str>) -> Result<(), CliError> {
    let settings = engine.settings().await?;
    let pinned = pinned(ctx);
    let fields = serde_json::to_value(&settings)
        .map_err(|e| EngineError::internal(format!("the settings: {e}")))?;
    let Some(fields) = fields.as_object() else {
        return Err(EngineError::internal("the settings are not an object").into());
    };
    if let Some(k) = key
        && !fields.contains_key(k)
    {
        let known: Vec<&str> = fields.keys().map(String::as_str).collect();
        return Err(EngineError::invalid_input(format!(
            "unknown setting {k:?} (known: {})",
            known.join(", ")
        ))
        .with_detail("kind", "unknownSetting")
        .into());
    }
    if ctx.out.is_json() {
        ctx.out.json(&SettingsView {
            dto: DTO_VERSION,
            settings: &settings,
            pinned,
        });
        return Ok(());
    }
    let width = fields.keys().map(String::len).max().unwrap_or(0);
    for (k, v) in fields {
        if key.is_some_and(|want| want != k) {
            continue;
        }
        let mut text = tt(&value_text(v));
        if pinned.contains(&k.as_str()) {
            text.append(lit(if ctx.config.sources.is_some() {
                " (pinned by config.toml)"
            } else {
                " (pinned by devnet mode)"
            }));
        }
        ctx.out.line(&[
            (tt(&format!("{k:<width$}")), Style::Bold),
            (lit("  "), Style::Plain),
            (text, Style::Plain),
        ]);
    }
    Ok(())
}

/// `lessdugin config get [key] | set <key> [value]`.
pub async fn config(ctx: &Ctx, args: ConfigArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    match args.action {
        ConfigAction::Get { key } => show(ctx, &engine, key.as_deref()).await,
        ConfigAction::Set { key, value } => {
            let v = value
                .as_deref()
                .map(|t| setting_value(&key, t, engine.host().now_ms()));
            engine.set_setting(&key, v.as_ref()).await?;
            if key == "sources" && pinned(ctx).contains(&"sources") {
                let devnet = ctx.config.sources.is_none();
                ctx.out.warn(cat([
                    lit(if devnet {
                        "devnet mode pins sources = direct"
                    } else {
                        "config.toml has pinned sources"
                    }),
                    lit(
                        ": the stored setting is kept but takes effect only once that pin is \
                         removed",
                    ),
                ]));
            }
            // `config get` has no `mediaAutoload`, `externalImages` and `mediaHosts`: the
            // engine's settings do not report the stored switches
            let shown = serde_json::to_value(engine.settings().await?)
                .ok()
                .and_then(|v| v.get(&key).cloned());
            match shown {
                Some(_) => show(ctx, &engine, Some(&key)).await,
                None => {
                    if ctx.out.is_json() {
                        show(ctx, &engine, None).await
                    } else {
                        ctx.out.line(&[
                            (tt(&key), Style::Bold),
                            (lit("  "), Style::Plain),
                            (
                                match &value {
                                    Some(v) => tt(v),
                                    None => lit("(default)"),
                                },
                                Style::Plain,
                            ),
                        ]);
                        Ok(())
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_as_set_setting_takes_them() {
        assert_eq!(
            setting_value("sources", "direct", 123),
            Value::String("direct".into())
        );
        assert_eq!(
            setting_value("mediaAutoloadHosts", "a.example, b.example", 123),
            serde_json::json!(["a.example", "b.example"])
        );
        assert_eq!(
            setting_value("mediaAutoloadHosts", "[\"a.example\"]", 123),
            serde_json::json!(["a.example"])
        );
        assert_eq!(
            setting_value("indexers", "https://idx.example", 123),
            serde_json::json!([{ "url": "https://idx.example", "grantedAtMs": 123 }])
        );
        assert_eq!(
            setting_value("externalImagesForums", "", 123),
            serde_json::json!([])
        );
        // a stored switch is its text
        assert_eq!(
            setting_value("mediaAutoload", "off", 123),
            Value::String("off".into())
        );
    }
}
