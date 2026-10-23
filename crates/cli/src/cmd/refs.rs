// SPDX-License-Identifier: AGPL-3.0-or-later
//! References on the command line (synthesis §6.10): listing indices (`3`, `c12`, a forum
//! number) are resolved here from this terminal's listings ([`crate::state`]); everything
//! else goes to the engine's `parse_ref` (at-uris, SPA links, `<handle>/<rkey>` in the current
//! forum). Forums are also accepted by alias or slug when they are followed.

use lessdugin_atp::syntax::{AtIdentifier, Did};
use lessdugin_core::discovery::{ForumKey, parse_forum_ref};
use lessdugin_core::dto::{Ref, RefContext};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_lexicon::collection::nsid;
use lessdugin_native::host::NativeHost;

use crate::ctx::{CliError, Ctx};

fn bad(msg: impl Into<String>) -> CliError {
    CliError::Engine(EngineError::invalid_ref(msg))
}

/// `n` (1-based) of a listing.
fn nth(list: &[String], n: usize, what: &str, again: &str) -> Result<String, CliError> {
    match n.checked_sub(1).and_then(|i| list.get(i)) {
        Some(u) => Ok(u.clone()),
        None if list.is_empty() => Err(bad(format!(
            "there is no {what} listing in this terminal yet; run `lessdugin {again}` first"
        ))),
        None => Err(bad(format!(
            "the last {what} listing in this terminal has {} entries, not {n}; run `lessdugin {again}` again",
            list.len()
        ))),
    }
}

fn index(s: &str) -> Option<usize> {
    (!s.is_empty() && s.len() <= 6 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// Resolves a post or comment reference (or any other the engine accepts).
pub async fn item(ctx: &Ctx, engine: &Engine<NativeHost>, input: &str) -> Result<Ref, CliError> {
    let t = input.trim();
    if let Some(n) = index(t) {
        let l = ctx.state.listings();
        return Ok(Ref::Post {
            uri: nth(&l.posts, n, "post", "ls")?,
        });
    }
    if let Some(n) = t.strip_prefix(['c', 'C']).and_then(index) {
        let l = ctx.state.listings();
        return Ok(Ref::Comment {
            uri: nth(&l.comments, n, "comment", "read <post>")?,
        });
    }
    let forum = if t.contains("://") || t.contains("#/") {
        None
    } else {
        current_opt(ctx, engine).await
    };
    Ok(engine.parse_ref(t, &RefContext { forum }).await?)
}

/// A post or comment uri (`invalid_ref` for anything else).
pub async fn post_or_comment(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    input: &str,
) -> Result<(String, bool), CliError> {
    match item(ctx, engine, input).await? {
        Ref::Post { uri } => Ok((uri, false)),
        Ref::Comment { uri } => Ok((uri, true)),
        Ref::Forum { .. } => Err(bad(format!(
            "{input:?} is a forum; list its posts with `lessdugin ls {input}`"
        ))),
        _ => Err(bad(format!("{input:?} is not a post or a comment"))),
    }
}

/// The canonical uri of a forum reference.
pub async fn forum(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    input: &str,
) -> Result<String, CliError> {
    let t = input.trim();
    if let Some(n) = index(t) {
        return nth(&ctx.state.listings().forums, n, "forum", "forum ls");
    }
    if t.contains('/') || t.contains("://") {
        let r = parse_forum_ref(t).map_err(|e| bad(e.to_string()))?;
        let owner: Did = match &r.owner {
            AtIdentifier::Did(d) => d.clone(),
            AtIdentifier::Handle(h) => {
                let at = format!("at://{h}/{}/{}", nsid::FORUM, r.slug);
                return match engine.parse_ref(&at, &RefContext::default()).await? {
                    Ref::Forum { uri } => Ok(uri),
                    _ => Err(bad(format!("{input:?} is not a forum"))),
                };
            }
        };
        let key = ForumKey::new(&owner, &r.slug).map_err(|e| bad(e.to_string()))?;
        return Ok(key.uri.to_string());
    }
    // an alias, else the slug of exactly one followed forum
    let want = t.trim_start_matches('@').to_lowercase();
    let followed = engine.followed().await?;
    if let Some(f) = followed
        .iter()
        .find(|f| f.alias.as_deref().is_some_and(|a| a.to_lowercase() == want))
    {
        return Ok(f.uri.clone());
    }
    let by_slug: Vec<&String> = followed
        .iter()
        .filter(|f| f.uri.rsplit('/').next() == Some(t))
        .map(|f| &f.uri)
        .collect();
    match by_slug.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(bad(format!(
            "{input:?} is not a forum reference (<handle>/<slug>, an at-uri or a link) nor the \
             alias or slug of a followed forum"
        ))),
        _ => Err(bad(format!(
            "several followed forums have the slug {input:?}; use <handle>/<slug>"
        ))),
    }
}

/// The current forum without asking the network: `--forum` (when it needs no lookup) or the
/// one set by `forum use`.
async fn current_opt(ctx: &Ctx, engine: &Engine<NativeHost>) -> Option<String> {
    if let Some(f) = &ctx.g.forum {
        return forum(ctx, engine, f).await.ok();
    }
    ctx.state.current_forum()
}

/// The forum to act in: `explicit`, else `--forum`, else the current forum (`forum use`),
/// else the only followed forum. A usage error when there is none.
pub async fn forum_or_current(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    explicit: Option<&str>,
) -> Result<String, CliError> {
    if let Some(f) = explicit.or(ctx.g.forum.as_deref()) {
        return forum(ctx, engine, f).await;
    }
    if let Some(f) = ctx.state.current_forum() {
        return Ok(f);
    }
    let followed = engine.followed().await?;
    if let [one] = followed.as_slice() {
        return Ok(one.uri.clone());
    }
    Err(CliError::usage(
        "which forum? Name one, pass --forum, or set the current forum with `lessdugin forum use \
         <forum>`",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices() {
        assert_eq!(index("3"), Some(3));
        assert_eq!(index("03"), Some(3));
        assert_eq!(index(""), None);
        assert_eq!(index("3a"), None);
        assert_eq!(index("1234567"), None);
        let l = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(nth(&l, 2, "post", "ls").unwrap(), "b");
        assert!(nth(&l, 0, "post", "ls").is_err());
        assert!(nth(&l, 3, "post", "ls").is_err());
        assert!(nth(&[], 1, "post", "ls").is_err());
    }
}
