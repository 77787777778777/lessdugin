// SPDX-License-Identifier: AGPL-3.0-or-later
//! `export [--mine|<forum>] [--format json|md] [--out dir]` (`synthesis-v1.md` §6.10,
//! `threat-model.md` PRIV-8): content, as JSON or markdown files.
//!
//! - **`--mine`** (the default, PRIV-8): the account's own posts and comments, every one its
//!   repository lists (the PDS's public `listRecords`, read anonymously as the engine's own-repo
//!   reads are), whatever forum they are in and whatever was done to them: they are the author's.
//! - **A forum**: what the forum shows, from the fold: every listed post (any tier) and the
//!   comments of its thread, **without hidden and deleted items** (PRIV-8), also for a moderator,
//!   whose views carry hidden content; placeholders of missing comments are left out too.
//! - **`--format json`** (the default): one `dto: 1` document, on stdout or as
//!   `<dir>/export.json`. Each item has its uri, CID, the markdown as written (a spilled body is
//!   fetched from its PDS) and the record as stored.
//! - **`--format md`**: files in `--out <dir>` (FILE-6: only where the user says): one per own post
//!   and comment (`posts/`, `comments/`), or one per thread of a forum with its comments, each
//!   with its fields as YAML frontmatter. File names come from the record keys, reduced to
//!   `[A-Za-z0-9._~-]`, so a key never names another directory.
//!
//! `export --local` (this device's local state) and `--import` are in `social.rs`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, Nsid};
use lessdugin_atp::xrpc::ListRecords;
use lessdugin_core::dto::{
    CommentSort, DTO_VERSION, ItemState, ListPostsQuery, PostFilter, PostSort, ThreadOpts,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_core::host::Host;
use lessdugin_core::limits;
use lessdugin_core::xrpc::{CallOpts, Service};
use lessdugin_lexicon::collection::nsid;
use lessdugin_native::host::NativeHost;
use serde::Serialize;
use serde_json::Value;

use crate::args::{ExportArgs, ExportFormatArg};
use crate::cmd::{read, refs, write};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, cat, lit, tt};

/// The largest spilled body read (`sync.md` §11.1: `textBlob` bodies are at most 1 MB).
const TEXT_BLOB_MAX_BYTES: u64 = 1_000_000;

/// At most this many `listRecords` pages per collection (100 records each).
const MINE_PAGES_MAX: usize = 10_000;

/// The content export (`--format json`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContentExport {
    dto: u32,
    /// `mine` or `forum`.
    scope: &'static str,
    /// The account (`mine`).
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    /// The forum (`forum`).
    #[serde(skip_serializing_if = "Option::is_none")]
    forum: Option<String>,
    exported_ms: i64,
    posts: Vec<Item>,
    comments: Vec<Item>,
}

/// One exported post or comment.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    uri: String,
    cid: String,
    author: String,
    /// The author's verified handle (forum exports).
    #[serde(skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    /// The text as written; absent when the body could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<String>,
    /// The record as stored.
    record: Value,
}

/// `lessdugin export [--mine|<forum>] [--format json|md] [--out dir]`.
pub async fn content(ctx: &Ctx, args: ExportArgs) -> Result<(), CliError> {
    if args.format == ExportFormatArg::Md && args.out.is_none() {
        return Err(CliError::usage(
            "a markdown export writes files: name a directory with --out",
        ));
    }
    let x = match args.forum.as_deref() {
        Some(f) => forum(ctx, f).await?,
        None => mine(ctx).await?,
    };
    // a spilled body is awaited to the end of its download (`fetch_blob`, capped by
    // `BLOB_TOTAL_TIMEOUT_MS`); one that still could not be read is said, not left out quietly
    for i in x.posts.iter().chain(&x.comments) {
        let spilled = i
            .record
            .get("body")
            .is_some_and(|b| b.get("textBlob").is_some());
        if i.markdown.is_none() && spilled {
            ctx.out.warn(cat([
                lit("the body of "),
                tt(&i.uri),
                lit(" could not be downloaded from its author's PDS: it is exported without \
                     its markdown"),
            ]));
        }
    }
    match (args.format, args.out.as_deref()) {
        (ExportFormatArg::Json, None) => {
            ctx.out.json(&x);
            Ok(())
        }
        (ExportFormatArg::Json, Some(dir)) => {
            make_dir(dir)?;
            let path = dir.join("export.json");
            let text = serde_json::to_string_pretty(&x)
                .map_err(|e| EngineError::internal(format!("the export: {e}")))?;
            write_file(&path, &text)?;
            done(ctx, &x, dir);
            Ok(())
        }
        (ExportFormatArg::Md, Some(dir)) => {
            make_dir(dir)?;
            match x.scope {
                "forum" => forum_md(&x, dir)?,
                _ => mine_md(&x, dir)?,
            }
            done(ctx, &x, dir);
            Ok(())
        }
        (ExportFormatArg::Md, None) => unreachable!("checked above"),
    }
}

fn done(ctx: &Ctx, x: &ContentExport, dir: &Path) {
    if ctx.out.is_json() {
        ctx.out.json(&serde_json::json!({
            "dto": DTO_VERSION,
            "posts": x.posts.len(),
            "comments": x.comments.len(),
            "path": dir.display().to_string(),
        }));
        return;
    }
    if !ctx.out.is_quiet() {
        ctx.out.line(&[(
            tt(&format!(
                "Exported {} posts and {} comments to {}.",
                x.posts.len(),
                x.comments.len(),
                dir.display()
            )),
            Style::Plain,
        )]);
    }
}

fn make_dir(dir: &Path) -> Result<(), CliError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        EngineError::invalid_input(format!("could not create {}: {e}", dir.display())).into()
    })
}

fn write_file(path: &Path, text: &str) -> Result<(), CliError> {
    std::fs::write(path, text).map_err(|e| {
        EngineError::invalid_input(format!("could not write {}: {e}", path.display())).into()
    })
}

// ---------------------------------------------------------------------------------------------
// The items

/// The markdown of a record's `body`: its inline text, or its spilled text fetched from `pds`
/// (CID-checked by the host); `None` when there is none or it cannot be read.
async fn markdown_of(
    engine: &Engine<NativeHost>,
    record: &Value,
    did: &str,
    pds: Option<&url::Url>,
) -> Option<String> {
    let body = record.get("body")?;
    if let Some(t) = body.get("text").and_then(Value::as_str) {
        return Some(t.to_owned());
    }
    let cid = body
        .get("textBlob")?
        .get("ref")?
        .get("$link")?
        .as_str()?
        .to_owned();
    let did = Did::parse(did).ok()?;
    let h = engine
        .host()
        .fetch_blob(pds?.clone(), did, cid, TEXT_BLOB_MAX_BYTES)
        .await
        .ok()?;
    let bytes = std::fs::read(&h.locator).ok()?;
    String::from_utf8(bytes).ok()
}

/// `--mine`: the account's posts and comments from its repository.
async fn mine(ctx: &Ctx) -> Result<ContentExport, CliError> {
    let account = ctx.account().await?;
    let engine = ctx.engine()?;
    if ctx.offline() {
        return Err(CliError::usage(
            "export --mine lists your repository at its PDS, which --offline does not allow",
        ));
    }
    let did = Did::parse(&account.did).map_err(|e| EngineError::internal(e.to_string()))?;
    let pds = url::Url::parse(&account.pds)
        .map_err(|e| EngineError::internal(format!("the account's PDS {}: {e}", account.pds)))?;
    let mut posts = Vec::new();
    let mut comments = Vec::new();
    for (coll, out) in [(nsid::POST, &mut posts), (nsid::COMMENT, &mut comments)] {
        let coll = Nsid::parse(coll).map_err(|e| EngineError::internal(e.to_string()))?;
        let mut cursor: Option<String> = None;
        for _ in 0..MINE_PAGES_MAX {
            let page = engine
                .xrpc()
                .query_endpoint(
                    &Service::Anon(pds.clone()),
                    &ListRecords {
                        repo: AtIdentifier::Did(did.clone()),
                        collection: coll.clone(),
                        limit: Some(100),
                        cursor: cursor.take(),
                        reverse: None,
                    },
                    &CallOpts::capped(limits::LIST_PAGE_MAX_BYTES),
                )
                .await
                .map_err(EngineError::from)?;
            for r in &page.records {
                let (Some(uri), Some(cid), Some(value)) = (&r.uri, &r.cid, &r.value) else {
                    continue;
                };
                // only the account's own records of the collection
                let Ok(u) = AtUri::parse(uri) else { continue };
                if u.did() != &did || u.collection() != &coll {
                    continue;
                }
                out.push(Item {
                    uri: uri.clone(),
                    cid: cid.clone(),
                    author: account.did.clone(),
                    handle: None,
                    markdown: markdown_of(engine, value, &account.did, Some(&pds)).await,
                    record: value.clone(),
                });
            }
            match page.cursor {
                Some(c) if !page.records.is_empty() => cursor = Some(c),
                _ => break,
            }
        }
    }
    posts.sort_by(|a, b| a.uri.cmp(&b.uri));
    comments.sort_by(|a, b| a.uri.cmp(&b.uri));
    Ok(ContentExport {
        dto: DTO_VERSION,
        scope: "mine",
        account: Some(account.did.clone()),
        forum: None,
        exported_ms: engine.host().now_ms(),
        posts,
        comments,
    })
}

/// True for an item a forum export leaves out (PRIV-8).
fn left_out(state: &ItemState) -> bool {
    matches!(
        state,
        ItemState::Hidden { .. } | ItemState::Tombstone { .. }
    )
}

/// A forum, as the fold shows it.
async fn forum(ctx: &Ctx, reference: &str) -> Result<ContentExport, CliError> {
    let engine = ctx.reader().await?;
    let uri = refs::forum(ctx, &engine, reference).await?;
    read::sync_forum_quietly(ctx, &engine, &uri).await;
    let mut pds_of = std::collections::BTreeMap::<String, Option<url::Url>>::new();
    let mut posts = Vec::new();
    let mut comments = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let list = engine
            .list_posts(&ListPostsQuery {
                forum: uri.clone(),
                sort: PostSort::New,
                filter: PostFilter::All,
                tag: None,
                kind: None,
                author: None,
                cursor: cursor.take(),
                limit: 0,
            })
            .await?;
        for p in &list.items {
            if left_out(&p.state) || !seen.insert(p.uri.clone()) {
                continue;
            }
            let thread = engine
                .get_thread(
                    &p.uri,
                    &ThreadOpts {
                        sort: CommentSort::Old,
                        unread_only: false,
                    },
                )
                .await?;
            posts.push(item(&engine, &mut pds_of, &p.uri, &p.cid, &p.author).await);
            for c in &thread.comments {
                if left_out(&c.state) || c.missing {
                    continue;
                }
                comments.push(item(&engine, &mut pds_of, &c.uri, &c.cid, &c.author).await);
            }
        }
        match list.cursor {
            Some(c) if !list.items.is_empty() => cursor = Some(c),
            _ => break,
        }
    }
    Ok(ContentExport {
        dto: DTO_VERSION,
        scope: "forum",
        account: None,
        forum: Some(uri),
        exported_ms: engine.host().now_ms(),
        posts,
        comments,
    })
}

/// One item of a forum export: its cached record (the version the view shows when it is the
/// cached one).
async fn item(
    engine: &Engine<NativeHost>,
    pds_of: &mut std::collections::BTreeMap<String, Option<url::Url>>,
    uri: &str,
    cid: &str,
    author: &lessdugin_core::dto::ActorView,
) -> Item {
    let (cid, record) = match write::cached(engine, uri).await {
        Ok((c, v)) => (c, v),
        Err(_) => (cid.to_owned(), Value::Null),
    };
    let markdown = match record.get("body").and_then(|b| b.get("text")) {
        Some(Value::String(t)) => Some(t.clone()),
        _ => {
            if !pds_of.contains_key(&author.did) {
                let pds = engine
                    .resolve_identity(&author.did)
                    .await
                    .ok()
                    .and_then(|r| r.identity.pds)
                    .and_then(|p| url::Url::parse(&p).ok());
                pds_of.insert(author.did.clone(), pds);
            }
            let pds = pds_of.get(&author.did).cloned().flatten();
            markdown_of(engine, &record, &author.did, pds.as_ref()).await
        }
    };
    Item {
        uri: uri.to_owned(),
        cid,
        author: author.did.clone(),
        handle: author.handle.clone(),
        markdown,
        record,
    }
}

// ---------------------------------------------------------------------------------------------
// Markdown files

/// A file name from a record key: `[A-Za-z0-9._~-]` kept, anything else `_`, and never a
/// leading dot (record keys are already restricted, `.` and `..` are not keys; this does not
/// rely on it).
fn file_stem(rkey: &str) -> String {
    let mut s: String = rkey
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(200)
        .collect();
    if s.is_empty() || s.starts_with('.') {
        s.insert(0, '_');
    }
    s
}

/// A unique `<stem>.md` in `dir` among `used`.
fn unique(dir: &Path, stem: &str, used: &mut BTreeSet<String>) -> PathBuf {
    let mut name = format!("{stem}.md");
    let mut n = 2;
    while !used.insert(name.clone()) {
        name = format!("{stem}-{n}.md");
        n += 1;
    }
    dir.join(name)
}

/// A YAML double-quoted scalar (JSON's string syntax is YAML's).
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// The frontmatter of an item.
fn front(it: &Item, post: bool) -> String {
    let r = &it.record;
    let mut f = String::from("---\n");
    if post {
        if let Some(t) = str_field(r, "title") {
            f.push_str(&format!("title: {}\n", q(t)));
        }
        f.push_str(&format!(
            "kind: {}\n",
            q(str_field(r, "kind").unwrap_or("article"))
        ));
        if let Some(l) = r.get("link").and_then(|l| str_field(l, "uri")) {
            f.push_str(&format!("link: {}\n", q(l)));
        }
        if let Some(tags) = r.get("tags").and_then(Value::as_array) {
            let tags: Vec<String> = tags.iter().filter_map(Value::as_str).map(q).collect();
            f.push_str(&format!("tags: [{}]\n", tags.join(", ")));
        }
    } else {
        if let Some(root) = r.get("root").and_then(|x| str_field(x, "uri")) {
            f.push_str(&format!("post: {}\n", q(root)));
        }
        if let Some(parent) = r.get("parent").and_then(|x| str_field(x, "uri")) {
            f.push_str(&format!("parent: {}\n", q(parent)));
        }
    }
    if let Some(forum) = str_field(r, "forum") {
        f.push_str(&format!("forum: {}\n", q(forum)));
    }
    f.push_str(&format!("uri: {}\n", q(&it.uri)));
    f.push_str(&format!("author: {}\n", q(&it.author)));
    if let Some(h) = &it.handle {
        f.push_str(&format!("handle: {}\n", q(h)));
    }
    if let Some(c) = str_field(r, "createdAt") {
        f.push_str(&format!("created: {}\n", q(c)));
    }
    f.push_str("---\n");
    f
}

/// The text of an item, or a note that it could not be read.
fn text_of(it: &Item) -> String {
    match &it.markdown {
        Some(t) => {
            let mut t = t.trim_end_matches('\n').to_owned();
            t.push('\n');
            t
        }
        None => "<!-- the text of this item could not be read -->\n".to_owned(),
    }
}

fn rkey_of(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or_default().to_owned()
}

/// `--mine --format md`: `posts/<rkey>.md` and `comments/<rkey>.md`.
fn mine_md(x: &ContentExport, dir: &Path) -> Result<(), CliError> {
    for (sub, items, post) in [("posts", &x.posts, true), ("comments", &x.comments, false)] {
        let d = dir.join(sub);
        make_dir(&d)?;
        let mut used = BTreeSet::new();
        for it in items.iter() {
            let path = unique(&d, &file_stem(&rkey_of(&it.uri)), &mut used);
            write_file(&path, &format!("{}{}", front(it, post), text_of(it)))?;
        }
    }
    Ok(())
}

/// A forum `--format md`: `<post rkey>.md` per thread, the post then its comments in thread
/// order.
fn forum_md(x: &ContentExport, dir: &Path) -> Result<(), CliError> {
    let mut used = BTreeSet::new();
    for p in &x.posts {
        let mut s = format!("{}{}", front(p, true), text_of(p));
        let thread: Vec<&Item> = x
            .comments
            .iter()
            .filter(|c| {
                c.record
                    .get("root")
                    .and_then(|r| str_field(r, "uri"))
                    .is_some_and(|r| r == p.uri)
            })
            .collect();
        if !thread.is_empty() {
            s.push_str("\n## Comments\n");
        }
        for c in thread {
            let who = c
                .handle
                .as_deref()
                .map_or_else(|| c.author.clone(), |h| format!("@{h}"));
            s.push_str(&format!(
                "\n### {} ({})\n\n<!-- {} -->\n\n{}",
                who,
                str_field(&c.record, "createdAt").unwrap_or(""),
                c.uri,
                text_of(c)
            ));
        }
        let path = unique(dir, &file_stem(&rkey_of(&p.uri)), &mut used);
        write_file(&path, &s)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_stay_in_the_directory() {
        assert_eq!(file_stem("3kabc-meta"), "3kabc-meta");
        assert_eq!(file_stem("a/b"), "a_b");
        assert_eq!(file_stem(".."), "_..");
        assert_eq!(file_stem(".hidden"), "_.hidden");
        assert_eq!(file_stem(""), "_");
        assert_eq!(file_stem("a:b\\c"), "a_b_c");
        let mut used = BTreeSet::new();
        let d = Path::new("out");
        assert_eq!(unique(d, "x", &mut used), d.join("x.md"));
        assert_eq!(unique(d, "x", &mut used), d.join("x-2.md"));
    }
}
