// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search, cached user pages, tags/wiki and sequence libraries.
//!
//! Reads use the synced engine views. Writes go through the ordinary outbox and are reported in
//! the same way as post and comment writes; JSON sequence drafts are parsed as engine DTOs.

use std::path::Path;

use lessdugin_core::dto::{
    BodyDraft, DTO_VERSION, ForumRole, PostSummary, SearchItem, SearchQuery, SequenceDraft,
    SequencePosition, SequenceSummary, SequenceView, TagSummary, TagView, UserView, WikiDraft,
};
use lessdugin_core::engine::{Engine, EngineError, ErrorCode};
use lessdugin_native::host::NativeHost;
use serde::Serialize;

use crate::args::{
    SearchArgs, SequenceAction, SequenceArgs, SequencePositionArgs, TagArgs, UserArgs, WikiAction,
    WikiArgs,
};
use crate::cmd::{draft, read, refs, write};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, TermText, cat, lit, name, tt};
use crate::render::{self, BodyStyle};

#[derive(Serialize)]
struct TagList<'a> {
    dto: u32,
    forum: &'a str,
    tags: &'a [TagSummary],
}

#[derive(Serialize)]
struct SequenceList<'a> {
    dto: u32,
    forum: &'a str,
    sequences: &'a [SequenceSummary],
}

fn joined(parts: Vec<TermText>) -> TermText {
    let mut out = TermText::empty();
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            out.append(tt(" · "));
        }
        out.append(part);
    }
    out
}

fn role(role: ForumRole) -> &'static str {
    match role {
        ForumRole::Owner => "owner",
        ForumRole::Admin => "admin",
        ForumRole::Moderator => "moderator",
        ForumRole::Member => "member",
        ForumRole::Established => "established",
        ForumRole::Newcomer => "newcomer",
        ForumRole::None => "none",
    }
}

fn body_style(ctx: &Ctx) -> BodyStyle {
    BodyStyle {
        color: ctx.out.color(),
        hyperlinks: ctx.out.color() && ctx.out.stdout_is_tty(),
        width: render::width(ctx.out.stdout_is_tty()),
    }
}

fn post_title(post: &PostSummary) -> TermText {
    match post
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    {
        Some(title) => tt(title),
        None => match post.excerpt.lines().next().filter(|line| !line.is_empty()) {
            Some(line) => tt(line),
            None => lit("(untitled)"),
        },
    }
}

fn print_post(ctx: &Ctx, post: &PostSummary) {
    ctx.out.line(&[(post_title(post), Style::Bold)]);
    let mut meta = vec![
        render::actor(&post.author),
        tt(&render::time(post.t_rank_ms)),
        render::count(post.comment_count, "comment", "comments"),
    ];
    meta.extend(render::score(&post.score));
    ctx.out.line(&[(joined(meta), Style::Dim)]);
    ctx.out.line(&[(tt(&post.uri), Style::Dim)]);
}

fn print_posts(ctx: &Ctx, heading: &'static str, posts: &[PostSummary]) {
    if posts.is_empty() {
        return;
    }
    ctx.out.line(&[(lit(heading), Style::Bold)]);
    for post in posts {
        print_post(ctx, post);
    }
}

/// `lessdugin search <text>`: only the local synced cache is inspected.
pub async fn search(ctx: &Ctx, args: SearchArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = match args.forum.as_deref() {
        Some(forum) => Some(refs::forum(ctx, &engine, forum).await?),
        None => None,
    };
    let results = engine
        .search(&SearchQuery {
            q: args.query,
            forum,
            author: args.author,
            limit: args.limit,
        })
        .await?;
    if ctx.out.is_json() {
        ctx.out.json(&results);
        return Ok(());
    }
    if results.hits.is_empty() {
        ctx.out
            .line(&[(lit("No matches in the local synced cache."), Style::Dim)]);
        return Ok(());
    }
    for hit in &results.hits {
        let (kind, title, uri) = match &hit.item {
            SearchItem::Post(post) => ("post", post_title(post), post.uri.as_str()),
            SearchItem::Comment(comment) => (
                "comment",
                cat([lit("comment on "), tt(&comment.root)]),
                comment.uri.as_str(),
            ),
        };
        ctx.out.line(&[
            (lit(kind), Style::Dim),
            (lit("  "), Style::Plain),
            (title, Style::Bold),
        ]);
        ctx.out.line(&[(tt(&hit.snippet), Style::Plain)]);
        ctx.out.line(&[(tt(uri), Style::Dim)]);
    }
    Ok(())
}

fn print_user(ctx: &Ctx, view: &UserView) {
    ctx.out.line(&[(render::actor(&view.actor), Style::Bold)]);
    ctx.out.field("did", 7, tt(&view.actor.did));
    if let Some(bio) = &view.bio {
        ctx.out.line(&[(lit("About"), Style::Bold)]);
        ctx.out
            .page(&render::view_body(bio, &body_style(ctx), 2), false);
    }
    for link in &view.links {
        let label = link.label.as_deref().unwrap_or(&link.host);
        ctx.out.line(&[
            (name(label), Style::Plain),
            (lit("  "), Style::Plain),
            (tt(&link.uri), Style::Dim),
        ]);
    }
    print_posts(ctx, "Pinned posts", &view.pinned);
    print_posts(ctx, "Recent posts", &view.recent_posts);
    if !view.recent_comments.is_empty() {
        ctx.out.line(&[(lit("Recent comments"), Style::Bold)]);
        for comment in &view.recent_comments {
            ctx.out.line(&[(tt(&comment.uri), Style::Bold)]);
            let excerpt = comment.body.text.lines().next().unwrap_or_default();
            ctx.out.line(&[(tt(excerpt), Style::Plain)]);
        }
    }
    if let Some(forum) = &view.forum {
        ctx.out.line(&[(lit("In forum"), Style::Bold)]);
        ctx.out.field("forum", 7, tt(forum));
        ctx.out.field("role", 7, lit(role(view.status)));
        if let Some(karma) = &view.karma {
            ctx.out.field(
                "karma",
                7,
                tt(&format!("{} (K1 {}, KP {})", karma.k, karma.k1, karma.kp)),
            );
        }
    }
}

/// `lessdugin user <actor> [--forum <forum>]`.
pub async fn user(ctx: &Ctx, args: UserArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = match args.forum.as_deref() {
        Some(forum) => Some(refs::forum(ctx, &engine, forum).await?),
        None => None,
    };
    let view = engine.user(forum.as_deref(), &args.actor).await?;
    if ctx.out.is_json() {
        ctx.out.json(&view);
    } else {
        print_user(ctx, &view);
    }
    Ok(())
}

fn print_tag_summary(ctx: &Ctx, tag: &TagSummary) {
    let mut meta = vec![render::count(tag.post_count, "post", "posts")];
    if tag.unmerged_edits > 0 {
        meta.push(cat([
            tt(&tag.unmerged_edits.to_string()),
            lit(" unmerged edits"),
        ]));
    }
    ctx.out.line(&[
        (cat([lit("#"), name(&tag.slug)]), Style::Bold),
        (lit("  "), Style::Plain),
        (name(&tag.name), Style::Plain),
    ]);
    ctx.out.line(&[(joined(meta), Style::Dim)]);
    if let Some(description) = &tag.description {
        ctx.out.line(&[(tt(description), Style::Plain)]);
    }
}

/// `lessdugin tag [slug] [--forum <forum>]`.
pub async fn tag(ctx: &Ctx, args: TagArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = refs::forum_or_current(ctx, &engine, args.forum.as_deref()).await?;
    match args.slug {
        None => {
            let tags = engine.tags(&forum).await?;
            if ctx.out.is_json() {
                ctx.out.json(&TagList {
                    dto: DTO_VERSION,
                    forum: &forum,
                    tags: &tags,
                });
            } else if tags.is_empty() {
                ctx.out.line(&[(lit("No tags."), Style::Dim)]);
            } else {
                for tag in &tags {
                    print_tag_summary(ctx, tag);
                }
            }
        }
        Some(slug) => {
            let mut view = engine.tag(&forum, &slug).await?;
            // one view per process: a spilled page the view left pending is awaited, as `read`
            // awaits a body (`limits.md` `BODY_BLOB_WAIT_MS`)
            let pending = |v: &TagView| v.page.as_ref().is_some_and(read::spilled_pending);
            if pending(&view)
                && let Some(head) = view.head.clone()
                && let Ok(author) = write::owner_of(&head)
                && read::await_spilled(ctx, &engine, "body", &[(head, author)]).await
            {
                view = engine.tag(&forum, &slug).await?;
            }
            if pending(&view) {
                ctx.out.warn(cat([
                    lit("the page of "),
                    tt(&slug),
                    lit(
                        " is still not loaded from its author's PDS (the excerpt shows); show \
                         it again later",
                    ),
                ]));
            }
            if ctx.out.is_json() {
                ctx.out.json(&view);
                return Ok(());
            }
            print_tag_summary(ctx, &view.tag);
            if let Some(head) = &view.head {
                ctx.out.field("revision", 8, tt(head));
            }
            if let Some(page) = &view.page {
                ctx.out
                    .page(&render::view_body(page, &body_style(ctx), 0), true);
            }
            print_posts(ctx, "Tagged posts", &view.posts);
        }
    }
    Ok(())
}

/// `lessdugin wiki edit …`.
///
/// The edit names the head it was made against (`WikiDraft.base`, decision O-2): `--base`, else
/// the page as this device last showed it (`tag <slug>` reads the same cache). The forum is
/// synced before the write, so a revision published since is seen and the edit refused
/// (`conflict`, `wikiHeadMoved`) rather than replacing it.
pub async fn wiki(ctx: &Ctx, args: WikiArgs) -> Result<(), CliError> {
    match args.action {
        WikiAction::Edit {
            slug,
            name,
            file,
            description,
            summary,
            tag,
            parent_tags,
            base,
            forum,
        } => {
            let markdown = draft::read_file(Path::new(&file))?;
            let w = ctx.writer().await?;
            let forum = refs::forum_or_current(ctx, &w.engine, forum.as_deref()).await?;
            let base = match base {
                Some(base) => Some(base),
                None => match w.engine.tag(&forum, &slug).await {
                    Ok(view) => view.head,
                    // a new page (or one that is no tag): nothing was loaded
                    Err(e) if e.code == ErrorCode::NotFound => None,
                    Err(e) => return Err(e.into()),
                },
            };
            read::sync_forum_quietly(ctx, &w.engine, &forum).await;
            let draft = WikiDraft {
                name,
                description,
                body: Some(BodyDraft::text(markdown.clone())),
                is_tag: tag,
                parent_tags,
                summary,
                base,
            };
            let ticket = match w.engine.wiki_edit(&forum, &slug, draft).await {
                Ok(ticket) => ticket,
                Err(e) if head_moved(&e) => {
                    return Err(head_moved_error(ctx, &w.engine, &slug, &file, &markdown, e));
                }
                Err(e) => return Err(e.into()),
            };
            let ticket = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &ticket, "Edited wiki")
        }
    }
}

/// `wiki_edit`'s refusal of an edit made against a head that is no longer the page's.
fn head_moved(e: &EngineError) -> bool {
    e.code == ErrorCode::Conflict
        && e.detail
            .as_ref()
            .and_then(|d| d.get("kind"))
            .and_then(serde_json::Value::as_str)
            == Some("wikiHeadMoved")
}

/// The `conflict` a refused wiki edit exits with: the page changed since it was loaded. The
/// author's text is kept (a file stays as it is; standard input is saved under the drafts
/// directory) and the message says how to reload the page and redo the edit on it.
fn head_moved_error(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    slug: &str,
    file: &str,
    markdown: &str,
    mut e: EngineError,
) -> CliError {
    let detail = |k: &str| {
        e.detail
            .as_ref()
            .and_then(|d| d.get(k))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let (base, head) = (detail("base"), detail("head"));
    let kept = if file == "-" {
        match keep_stdin_text(ctx, engine, markdown) {
            Ok(path) => Some(path.display().to_string()),
            Err(why) => {
                ctx.out.warn(cat([
                    lit("your text could not be kept: "),
                    tt(&why.to_string()),
                ]));
                None
            }
        }
    } else {
        Some(file.to_owned())
    };
    let mut message = format!("the page {slug} changed since it was loaded");
    if let Some(base) = &base {
        message.push_str(&format!(" (your edit was made against {base}"));
        match &head {
            Some(head) => message.push_str(&format!(", the page is now at {head})")),
            None => message.push(')'),
        }
    }
    message.push_str(", so nothing was written; ");
    match &kept {
        Some(path) => message.push_str(&format!("your text is kept in {path}. ")),
        None => message.push_str("your text was not kept. "),
    }
    message.push_str(&format!(
        "Reload the page with `lessdugin tag {slug}`, redo your edit on it, then run `lessdugin \
         wiki edit {slug} --base {} --file <your file> …` again",
        head.as_deref().unwrap_or("<the revision it shows>")
    ));
    e.message = message;
    if let Some(path) = kept
        && let Some(serde_json::Value::Object(d)) = &mut e.detail
    {
        d.insert("kept".into(), path.into());
    }
    CliError::Engine(e)
}

/// Saves a refused wiki edit's text read from standard input (`drafts/wiki/<id>.md`, private,
/// outside the drafts `draft` lists) and returns its path.
fn keep_stdin_text(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    markdown: &str,
) -> std::io::Result<std::path::PathBuf> {
    let drafts = draft::Drafts::new(ctx);
    let dir = ctx.paths.drafts_dir().join("wiki");
    lessdugin_native::fsutil::ensure_private_dir(&ctx.paths.drafts_dir())?;
    lessdugin_native::fsutil::ensure_private_dir(&dir)?;
    let path = loop {
        let path = dir.join(format!("{}.md", drafts.new_id(engine.host())));
        if !path.exists() {
            break path;
        }
    };
    lessdugin_native::fsutil::write_private_atomic(&path, markdown.as_bytes())?;
    Ok(path)
}

fn parse_sequence_draft(text: &str, path: &Path) -> Result<SequenceDraft, CliError> {
    serde_json::from_str(text).map_err(|e| {
        EngineError::invalid_input(format!("invalid SequenceDraft in {}: {e}", path.display()))
            .into()
    })
}

fn read_sequence_draft(path: &Path) -> Result<SequenceDraft, CliError> {
    parse_sequence_draft(&draft::read_file(path)?, path)
}

fn position(at: SequencePositionArgs) -> SequencePosition {
    SequencePosition {
        chapter: at.chapter,
        index: at.index,
    }
}

async fn canonical_draft_forum(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    draft: &mut SequenceDraft,
) -> Result<(), CliError> {
    draft.forum = refs::forum(ctx, engine, &draft.forum).await?;
    Ok(())
}

fn print_sequence(ctx: &Ctx, view: &SequenceView) {
    let sequence = &view.sequence;
    ctx.out.line(&[(tt(&sequence.title), Style::Bold)]);
    ctx.out
        .line(&[(render::actor(&sequence.author), Style::Dim)]);
    ctx.out.line(&[(tt(&sequence.uri), Style::Dim)]);
    if let Some(body) = &view.body {
        ctx.out
            .page(&render::view_body(body, &body_style(ctx), 0), true);
    }
    for (i, chapter) in view.chapters.iter().enumerate() {
        let heading = chapter
            .title
            .as_deref()
            .map_or_else(|| format!("Chapter {}", i + 1), |title| title.to_owned());
        ctx.out.line(&[(tt(&heading), Style::Bold)]);
        if let Some(body) = &chapter.body {
            ctx.out
                .page(&render::view_body(body, &body_style(ctx), 2), false);
        }
        for post in &chapter.posts {
            print_post(ctx, post);
        }
    }
}

/// `lessdugin sequence …`.
pub async fn sequence(ctx: &Ctx, args: SequenceArgs) -> Result<(), CliError> {
    match args.action {
        SequenceAction::Ls { forum } => {
            let engine = ctx.reader().await?;
            let forum = refs::forum_or_current(ctx, &engine, forum.as_deref()).await?;
            let sequences = engine.sequences(&forum).await?;
            if ctx.out.is_json() {
                ctx.out.json(&SequenceList {
                    dto: DTO_VERSION,
                    forum: &forum,
                    sequences: &sequences,
                });
            } else if sequences.is_empty() {
                ctx.out.line(&[(lit("No sequences."), Style::Dim)]);
            } else {
                for sequence in &sequences {
                    ctx.out.line(&[(tt(&sequence.title), Style::Bold)]);
                    let mut meta = vec![
                        render::actor(&sequence.author),
                        render::count(sequence.chapters, "chapter", "chapters"),
                        render::count(sequence.posts, "post", "posts"),
                    ];
                    meta.extend(render::score(&sequence.score));
                    if sequence.pinned {
                        meta.push(lit("pinned"));
                    }
                    ctx.out.line(&[(joined(meta), Style::Dim)]);
                    ctx.out.line(&[(tt(&sequence.uri), Style::Dim)]);
                }
            }
            Ok(())
        }
        SequenceAction::Read { uri } => {
            let engine = ctx.reader().await?;
            let mut view = engine.sequence(&uri).await?;
            // as `tag <slug>` awaits its page
            let pending = |v: &SequenceView| v.body.as_ref().is_some_and(read::spilled_pending);
            if pending(&view) {
                let author = [(view.sequence.uri.clone(), view.sequence.author.did.clone())];
                if read::await_spilled(ctx, &engine, "body", &author).await {
                    view = engine.sequence(&uri).await?;
                }
            }
            if pending(&view) {
                ctx.out.warn(cat([
                    lit("the body of "),
                    tt(&view.sequence.uri),
                    lit(
                        " is still not loaded from its author's PDS (the excerpt shows); read \
                         it again later",
                    ),
                ]));
            }
            if ctx.out.is_json() {
                ctx.out.json(&view);
            } else {
                print_sequence(ctx, &view);
            }
            Ok(())
        }
        SequenceAction::Create { file } => {
            let mut draft = read_sequence_draft(Path::new(&file))?;
            let w = ctx.writer().await?;
            canonical_draft_forum(ctx, &w.engine, &mut draft).await?;
            let ticket = w.engine.seq_create(&draft).await?;
            let ticket = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &ticket, "Created")
        }
        SequenceAction::Edit { uri, file } => {
            let mut draft = read_sequence_draft(Path::new(&file))?;
            let w = ctx.writer_for(&write::owner_of(&uri)?).await?;
            canonical_draft_forum(ctx, &w.engine, &mut draft).await?;
            let ticket = w.engine.seq_edit(&uri, &draft).await?;
            let ticket = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &ticket, "Edited")
        }
        SequenceAction::Add { uri, post, at } => {
            let w = ctx.writer_for(&write::owner_of(&uri)?).await?;
            let ticket = w.engine.seq_add(&uri, &post, &position(at)).await?;
            let ticket = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &ticket, "Added")
        }
        SequenceAction::Remove { uri, post } => {
            let w = ctx.writer_for(&write::owner_of(&uri)?).await?;
            let ticket = w.engine.seq_remove(&uri, &post).await?;
            let ticket = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &ticket, "Removed")
        }
        SequenceAction::Move {
            uri,
            from,
            to_chapter,
            to_index,
        } => {
            let w = ctx.writer_for(&write::owner_of(&uri)?).await?;
            let to = SequencePosition {
                chapter: to_chapter,
                index: to_index,
            };
            let ticket = w.engine.seq_move(&uri, &position(from), &to).await?;
            let ticket = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &ticket, "Moved")
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::args::{Cli, Command};

    #[test]
    fn organisation_commands_dispatch() {
        let cli = Cli::try_parse_from([
            "lessdugin",
            "search",
            "borrowed",
            "--forum",
            "alice.example/meta",
            "--author",
            "bob.example",
            "--limit",
            "7",
        ])
        .unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("search did not dispatch");
        };
        assert_eq!(args.query, "borrowed");
        assert_eq!(args.limit, 7);

        let cli = Cli::try_parse_from([
            "lessdugin",
            "sequence",
            "move",
            "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1",
            "--chapter",
            "1",
            "--index",
            "2",
            "--to-chapter",
            "3",
            "--to-index",
            "4",
        ])
        .unwrap();
        let Some(Command::Sequence(SequenceArgs {
            action:
                SequenceAction::Move {
                    from,
                    to_chapter,
                    to_index,
                    ..
                },
        })) = cli.command
        else {
            panic!("sequence move did not dispatch");
        };
        assert_eq!(
            (from.chapter, from.index, to_chapter, to_index),
            (1, 2, 3, 4)
        );

        let cli = Cli::try_parse_from([
            "lessdugin",
            "wiki",
            "edit",
            "rust",
            "--name",
            "Rust",
            "--file",
            "page.md",
            "--tag",
            "false",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Wiki(WikiArgs {
                action: WikiAction::Edit { tag: false, .. }
            }))
        ));
    }

    #[test]
    fn sequence_drafts_are_typed_and_errors_stay_errors() {
        let draft = parse_sequence_draft(
            r#"{
                "forum":"at://did:plc:a/online.7778777.lessdugin.forum/meta",
                "title":"Reading",
                "body":{"markdown":"Introduction"},
                "chapters":[{"title":"One","posts":[],"body":{"markdown":"Chapter"}}]
            }"#,
            Path::new("sequence.json"),
        )
        .unwrap();
        assert_eq!(draft.title, "Reading");
        assert_eq!(draft.body.unwrap().markdown, "Introduction");
        assert_eq!(draft.chapters[0].body.as_ref().unwrap().markdown, "Chapter");
        assert!(parse_sequence_draft("not json", Path::new("bad.json")).is_err());
        assert!(parse_sequence_draft("{}", Path::new("missing.json")).is_err());
        assert!(
            parse_sequence_draft(
                r#"{"forum":"f","title":"t","chapters":"no"}"#,
                Path::new("wrong.json")
            )
            .is_err()
        );
    }

    #[test]
    fn search_plain_text_has_no_terminal_controls() {
        let rendered = crate::out::render(
            &tt("match \u{1b}]0;title\u{7}\u{202e}snippet"),
            Style::Plain,
            false,
        );
        for control in ['\u{1b}', '\u{7}', '\u{202e}'] {
            assert!(!rendered.contains(control), "{rendered:?}");
        }
        assert!(rendered.contains("snippet"));
    }
}
