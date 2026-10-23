// SPDX-License-Identifier: AGPL-3.0-or-later
//! Drafts and the editor (synthesis §6.10 "Drafts").
//!
//! A draft is `$DATA/drafts/<id>.md`: YAML frontmatter, then the markdown body.
//!
//! ```text
//! ---
//! type: post                       # post | comment | edit
//! forum: "at://did:plc:…/…forum/meta"
//! title: "Hello"
//! kind: article                    # article | link | question | quickTake
//! link: "https://example.com"      # link posts
//! tags: ["intro", "meta"]
//! ---
//! The body, in markdown.
//! ```
//!
//! Comments have `root` (the post) and `parent` (the comment replied to); edits have `target`
//! (the record) and `summary`. Only this flat subset of YAML is read: `key: value` lines with a
//! bare, double-quoted (JSON escapes) or single-quoted string, or a `[…]` list of them; `#`
//! starts a comment line. Validation errors are written back as single-line `# error: …`
//! comments under the opening `---`, so the next editor session shows them.
//!
//! **Editor safety (`threat-model.md` TERM-8).** A draft file is opened in `$EDITOR`, and its
//! fields and body may hold text from the network (an edited record, an error message quoting
//! a server). The file never lets that text configure the editor: the `# error:` lines are
//! sanitised (`term_sanitize(Prose)`, one line) and their modelines neutralised; four fixed
//! comment lines follow them, so no field is in the first five lines; and the body is followed
//! by the [`SCISSORS`] line and five fixed lines, so it is never in the last five (vim reads
//! modelines there). Everything from the scissors line down is ignored when the file is read.
//!
//! The editor is the first of `$VISUAL`, `$EDITOR`, `editor`, `nano`, `vi` that can be started
//! (`notepad` on Windows); `$VISUAL`/`$EDITOR` may carry arguments (`code --wait`).

use std::path::{Path, PathBuf};

use lessdugin_core::dto::{BodyDraft, ExternalLinkDraft, PostDraft, PostKind};
use lessdugin_core::engine::EngineError;
use lessdugin_core::host::Host;
use lessdugin_native::fsutil::{ensure_private_dir, read_private, write_private_atomic};
use lessdugin_native::template::{SCISSORS, neutralise_modelines};
use serde::Serialize;

use crate::args::{DraftAction, DraftArgs};
use crate::cmd::{refs, write};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, cat, lit, tt};
use crate::prompt;
use crate::render;

/// The largest draft read.
const DRAFT_MAX_BYTES: u64 = 2 * 1024 * 1024;

const ERROR_PREFIX: &str = "# error: ";

/// The fixed lines under the opening `---` (after any `# error:` lines): with them no field is
/// in the first five lines of the file.
const HEAD: [&str; 4] = [
    "# A lessdugin draft: the fields below, then the text after the closing `---`.",
    "# Lines starting with # are ignored here.",
    "#",
    "#",
];

/// The fixed lines after the body: the scissors line and five more, so the body is never in
/// the last five lines of the file.
const TAIL: [&str; 5] = [
    "# Everything from the line above down is ignored.",
    "#",
    "#",
    "#",
    "# lessdugin",
];

/// What a draft is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftType {
    /// A new post.
    Post,
    /// A comment or reply.
    Comment,
    /// An edit of a post or comment.
    Edit,
}

impl DraftType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Comment => "comment",
            Self::Edit => "edit",
        }
    }
}

/// A frontmatter value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A string.
    Str(String),
    /// A list of strings.
    List(Vec<String>),
}

/// A parsed draft document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doc {
    /// Frontmatter fields in order.
    pub fields: Vec<(String, Value)>,
    /// The markdown body.
    pub body: String,
}

fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn unquote(v: &str) -> Result<String, String> {
    let v = v.trim();
    if v.starts_with('"') {
        return serde_json::from_str::<String>(v).map_err(|e| format!("bad string {v}: {e}"));
    }
    if let Some(inner) = v.strip_prefix('\'') {
        let inner = inner
            .strip_suffix('\'')
            .ok_or_else(|| format!("unterminated string {v}"))?;
        return Ok(inner.replace("''", "'"));
    }
    Ok(v.to_owned())
}

fn parse_list(v: &str) -> Result<Vec<String>, String> {
    let inner = v
        .trim()
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .ok_or_else(|| format!("bad list {v}"))?;
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&format!("[{inner}]")) {
        return Ok(list);
    }
    inner
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(unquote)
        .collect()
}

/// One `# error:` line: sanitised for the terminal (no controls, line or paragraph separators
/// or bidi controls), on one line, with editor modelines neutralised.
fn error_line(e: &str) -> String {
    let one_line = tt(e).to_plain_string().replace(['\n', '\r'], " ");
    neutralise_modelines(&one_line)
}

/// `body` without the scissors line and everything after it.
fn without_tail(body: &str) -> String {
    let mut at = 0;
    for line in body.split_inclusive('\n') {
        if line.trim_end() == SCISSORS {
            return body[..at].to_owned();
        }
        at += line.len();
    }
    body.to_owned()
}

impl Doc {
    /// Parses a document; a file that does not start with `---` is all body. The scissors line
    /// and everything after it are not part of the body.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = text.split_inclusive('\n');
        let first = lines.clone().next().unwrap_or_default();
        if first.trim_end() != "---" {
            return Ok(Self {
                fields: Vec::new(),
                body: without_tail(text),
            });
        }
        lines.next();
        let mut fields = Vec::new();
        let mut consumed = first.len();
        let mut closed = false;
        for l in lines {
            consumed += l.len();
            let t = l.trim_end();
            if t == "---" || t == "..." {
                closed = true;
                break;
            }
            let t = t.trim_start();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let (k, v) = t
                .split_once(':')
                .ok_or_else(|| format!("not a `key: value` line: {t}"))?;
            let k = k.trim().to_owned();
            if k.is_empty()
                || !k
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(format!("bad key {k:?}"));
            }
            let v = v.trim();
            let v = if v.starts_with('[') {
                Value::List(parse_list(v)?)
            } else {
                Value::Str(unquote(v)?)
            };
            fields.push((k, v));
        }
        if !closed {
            return Err("the frontmatter is not closed with a `---` line".into());
        }
        Ok(Self {
            fields,
            body: without_tail(&text[consumed..]),
        })
    }

    /// The text of the document, with `errors` as comments under the opening `---` (see the
    /// module docs on editor safety).
    pub fn render(&self, errors: &[String]) -> String {
        let mut s = String::from("---\n");
        for e in errors {
            s.push_str(ERROR_PREFIX);
            s.push_str(&error_line(e));
            s.push('\n');
        }
        for l in HEAD {
            s.push_str(l);
            s.push('\n');
        }
        for (k, v) in &self.fields {
            s.push_str(k);
            s.push_str(": ");
            match v {
                Value::Str(x) if is_bare(x) => s.push_str(x),
                Value::Str(x) => s.push_str(&yaml_str(x)),
                Value::List(xs) => {
                    let items: Vec<String> = xs.iter().map(|x| yaml_str(x)).collect();
                    s.push('[');
                    s.push_str(&items.join(", "));
                    s.push(']');
                }
            }
            s.push('\n');
        }
        s.push_str("---\n");
        s.push_str(&self.body);
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(SCISSORS);
        s.push('\n');
        for l in TAIL {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    /// A string field (`None` when absent or empty).
    pub fn get(&self, k: &str) -> Option<&str> {
        self.fields.iter().find_map(|(key, v)| match v {
            Value::Str(s) if key == k && !s.trim().is_empty() => Some(s.as_str()),
            _ => None,
        })
    }

    /// A list field (a single string counts as a one-item list).
    pub fn list(&self, k: &str) -> Vec<String> {
        self.fields
            .iter()
            .find_map(|(key, v)| (key == k).then_some(v))
            .map(|v| match v {
                Value::List(xs) => xs.clone(),
                Value::Str(s) if s.trim().is_empty() => Vec::new(),
                Value::Str(s) => vec![s.clone()],
            })
            .unwrap_or_default()
    }

    /// Sets a field (replacing it in place, or appending it).
    pub fn set(&mut self, k: &str, v: Value) {
        match self.fields.iter_mut().find(|(key, _)| key == k) {
            Some((_, old)) => *old = v,
            None => self.fields.push((k.to_owned(), v)),
        }
    }

    /// Sets a string field, or removes it for `None`.
    pub fn set_opt(&mut self, k: &str, v: Option<&str>) {
        match v {
            Some(v) => self.set(k, Value::Str(v.to_owned())),
            None => self.fields.retain(|(key, _)| key != k),
        }
    }

    /// The draft type (`post` when absent).
    pub fn draft_type(&self) -> Result<DraftType, String> {
        match self.get("type").unwrap_or("post") {
            "post" => Ok(DraftType::Post),
            "comment" => Ok(DraftType::Comment),
            "edit" => Ok(DraftType::Edit),
            other => Err(format!("unknown type {other:?} (post, comment or edit)")),
        }
    }

    /// The body, without surrounding blank lines.
    pub fn body_text(&self) -> String {
        self.body.trim_matches(['\n', '\r']).to_owned() + "\n"
    }

    /// True if the body has no text.
    pub fn body_is_empty(&self) -> bool {
        self.body.trim().is_empty()
    }
}

fn is_bare(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// The post kind of a frontmatter value.
pub fn parse_kind(s: &str) -> Result<PostKind, String> {
    match s {
        "article" => Ok(PostKind::Article),
        "link" => Ok(PostKind::Link),
        "question" => Ok(PostKind::Question),
        "quickTake" | "quick-take" | "quick_take" => Ok(PostKind::QuickTake),
        other => Err(format!(
            "unknown kind {other:?} (article, link, question or quickTake)"
        )),
    }
}

/// The frontmatter value of a post kind.
pub fn kind_str(k: PostKind) -> &'static str {
    match k {
        PostKind::Article => "article",
        PostKind::Link => "link",
        PostKind::Question => "question",
        PostKind::QuickTake => "quickTake",
    }
}

/// The checks the CLI makes before handing a post to the engine (which validates the record
/// itself): `forum` must already be resolved.
pub fn post_draft(doc: &Doc, forum: Option<String>) -> Result<PostDraft, Vec<String>> {
    let mut errors = Vec::new();
    let kind = match doc.get("kind").map(parse_kind).transpose() {
        Ok(k) => k.unwrap_or(PostKind::Article),
        Err(e) => {
            errors.push(e);
            PostKind::Article
        }
    };
    if forum.is_none() {
        errors.push("no forum: add `forum: <handle>/<slug>` or pass --forum".into());
    }
    let title = doc.get("title").map(|t| t.trim().to_owned());
    if title.is_none() && kind != PostKind::QuickTake {
        errors.push(format!(
            "a {} needs a title (`title: …`)",
            match kind {
                PostKind::Link => "link post",
                PostKind::Question => "question",
                _ => "post",
            }
        ));
    }
    let link = doc.get("link").or_else(|| doc.get("url"));
    if kind == PostKind::Link && link.is_none() {
        errors.push("a link post needs `link: https://…`".into());
    }
    if let Some(l) = link
        && !(l.starts_with("https://") || l.starts_with("http://"))
    {
        errors.push(format!("the link {l:?} is not an http(s) URL"));
    }
    if doc.body_is_empty() && kind != PostKind::Link {
        errors.push("the body is empty".into());
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(PostDraft {
        forum: forum.unwrap_or_default(),
        kind,
        title,
        body: BodyDraft::text(if doc.body_is_empty() {
            String::new()
        } else {
            doc.body_text()
        }),
        link: link.map(|uri| ExternalLinkDraft {
            uri: uri.to_owned(),
            title: None,
            description: None,
        }),
        tags: doc.list("tags"),
        labels: Vec::new(),
    })
}

// ---------------------------------------------------------------------------------------------
// The draft store

/// The draft files of one data directory.
pub struct Drafts {
    dir: PathBuf,
}

/// A valid draft id: 1–32 of `[a-z0-9-]`.
fn check_id(id: &str) -> Result<(), CliError> {
    if !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Ok(())
    } else {
        Err(CliError::Engine(EngineError::invalid_input(format!(
            "{id:?} is not a draft id"
        ))))
    }
}

fn io_error(what: &str, path: &Path, e: &std::io::Error) -> CliError {
    CliError::Engine(EngineError::internal(format!(
        "{what} {}: {e}",
        path.display()
    )))
}

impl Drafts {
    /// The drafts of `ctx`'s data directory.
    pub fn new(ctx: &Ctx) -> Self {
        Self {
            dir: ctx.paths.drafts_dir(),
        }
    }

    /// The file of draft `id`.
    pub fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    /// A new, unused draft id (6 random characters).
    pub fn new_id<H: Host>(&self, host: &H) -> String {
        const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
        loop {
            let mut buf = [0u8; 6];
            host.fill_random(&mut buf);
            let id: String = buf
                .iter()
                .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
                .collect();
            if !self.path(&id).exists() {
                return id;
            }
        }
    }

    /// Writes a draft.
    pub fn save(&self, id: &str, doc: &Doc, errors: &[String]) -> Result<PathBuf, CliError> {
        check_id(id)?;
        ensure_private_dir(&self.dir).map_err(|e| io_error("creating", &self.dir, &e))?;
        let path = self.path(id);
        write_private_atomic(&path, doc.render(errors).as_bytes())
            .map_err(|e| io_error("writing", &path, &e))?;
        Ok(path)
    }

    /// Reads and parses a draft.
    pub fn load(&self, id: &str) -> Result<Doc, CliError> {
        check_id(id)?;
        let path = self.path(id);
        let text = read_file(&path)?;
        Doc::parse(&text).map_err(|e| {
            CliError::Engine(EngineError::invalid_input(format!(
                "draft {id}: {e} (fix it with `lessdugin draft edit {id}`)"
            )))
        })
    }

    /// Deletes a draft (a missing one is fine).
    pub fn remove(&self, id: &str) -> Result<(), CliError> {
        check_id(id)?;
        let path = self.path(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error("deleting", &path, &e)),
        }
    }

    /// Every draft id with its modification time (ms), newest first.
    pub fn list(&self) -> Vec<(String, i64)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<(String, i64)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".md")?.to_owned();
                check_id(&id).ok()?;
                let ms = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                Some((id, ms))
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

/// Reads a text file (`-`: standard input).
pub fn read_file(path: &Path) -> Result<String, CliError> {
    if path.as_os_str() == "-" {
        return read_limited(std::io::stdin().lock(), path);
    }
    let bytes = read_private(path, DRAFT_MAX_BYTES, false)
        .map_err(|e| io_error("reading", path, &e))?
        .ok_or_else(|| {
            CliError::Engine(EngineError::not_found(format!(
                "{} does not exist",
                path.display()
            )))
        })?;
    String::from_utf8(bytes).map_err(|_| {
        CliError::Engine(EngineError::invalid_input(format!(
            "{} is not UTF-8 text",
            path.display()
        )))
    })
}

/// Reads at most [`DRAFT_MAX_BYTES`] of UTF-8 text from `r`; more is an error (never a
/// silently cut text).
fn read_limited(r: impl std::io::Read, path: &Path) -> Result<String, CliError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    r.take(DRAFT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("reading", path, &e))?;
    if bytes.len() as u64 > DRAFT_MAX_BYTES {
        return Err(CliError::Engine(
            EngineError::new(
                lessdugin_core::engine::ErrorCode::TooLarge,
                format!(
                    "{} is larger than {DRAFT_MAX_BYTES} bytes; nothing was read",
                    path.display()
                ),
            )
            .with_detail("maxBytes", DRAFT_MAX_BYTES),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        CliError::Engine(EngineError::invalid_input(format!(
            "{} is not UTF-8 text",
            path.display()
        )))
    })
}

/// The editor commands to try, in order.
fn editor_chain() -> Vec<String> {
    let mut v: Vec<String> = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .filter(|s| !s.trim().is_empty())
        .collect();
    if cfg!(windows) {
        v.push("notepad".into());
    } else {
        v.extend(["editor", "nano", "vi"].map(String::from));
    }
    v
}

/// Opens `path` in the editor and waits for it.
pub fn edit_file(path: &Path) -> Result<(), CliError> {
    for cmd in editor_chain() {
        let mut parts = cmd.split_whitespace();
        let Some(program) = parts.next() else {
            continue;
        };
        let status = std::process::Command::new(program)
            .args(parts)
            .arg(path)
            .status();
        match status {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => {
                return Err(CliError::usage(format!(
                    "the editor ({program}) exited with {s}"
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CliError::usage(format!(
                    "could not start the editor ({program}): {e}"
                )));
            }
        }
    }
    Err(CliError::usage(
        "no editor found; set $VISUAL or $EDITOR, or pass a file",
    ))
}

/// Runs the editor on draft `id`, which stays where it is if the editor fails (the error says
/// so).
fn edit_kept(drafts: &Drafts, id: &str) -> Result<(), CliError> {
    edit_file(&drafts.path(id)).map_err(|e| match e {
        CliError::Usage(m) => CliError::Usage(format!(
            "{m}; the draft is kept (`lessdugin draft edit {id}`)"
        )),
        e => e,
    })
}

/// What to do after the publish summary.
enum Answer {
    Yes,
    No,
    Edit,
}

fn ask_publish() -> Answer {
    match prompt::read_line("Publish? [y/N/e] ") {
        Ok(Some(a)) => match a.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Answer::Yes,
            "e" | "edit" => Answer::Edit,
            _ => Answer::No,
        },
        _ => Answer::No,
    }
}

fn summary(ctx: &Ctx, d: &PostDraft, forum_name: &str) {
    let words = lessdugin_markdown::derive_metadata(&d.body.markdown, &[]).word_count;
    let out = &ctx.out;
    out.err_line(&[(lit("About to publish:"), Style::Bold)]);
    out.err_line(&[
        (lit("  forum  "), Style::Dim),
        (tt(forum_name), Style::Plain),
    ]);
    out.err_line(&[
        (lit("  kind   "), Style::Dim),
        (lit(kind_str(d.kind)), Style::Plain),
    ]);
    if let Some(t) = &d.title {
        out.err_line(&[(lit("  title  "), Style::Dim), (tt(t), Style::Plain)]);
    }
    if let Some(l) = &d.link {
        out.err_line(&[(lit("  link   "), Style::Dim), (tt(&l.uri), Style::Plain)]);
    }
    if !d.tags.is_empty() {
        out.err_line(&[
            (lit("  tags   "), Style::Dim),
            (tt(&d.tags.join(", ")), Style::Plain),
        ]);
    }
    out.err_line(&[
        (lit("  words  "), Style::Dim),
        (tt(&words.to_string()), Style::Plain),
    ]);
}

/// Where a post comes from.
pub struct PostSource {
    /// The document.
    pub doc: Doc,
    /// Its draft id, once it is saved as a draft.
    pub draft: Option<String>,
}

/// Saves `src` as a draft (a new id the first time); returns the id.
fn keep(
    ctx: &Ctx,
    drafts: &Drafts,
    src: &mut PostSource,
    errors: &[String],
) -> Result<String, CliError> {
    let id = match &src.draft {
        Some(id) => id.clone(),
        None => drafts.new_id(ctx.engine()?.host()),
    };
    drafts.save(&id, &src.doc, errors)?;
    src.draft = Some(id.clone());
    Ok(id)
}

/// The publish loop of `post` and `draft publish`: validate, show the summary and ask
/// `[y/N/e]` (unless `yes`), publish, send, report. A draft that cannot be published (errors,
/// no answer) is kept with its errors as comments.
pub async fn publish_post(ctx: &Ctx, mut src: PostSource, yes: bool) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let drafts = Drafts::new(ctx);
    let interactive = prompt::stdin_is_tty();
    loop {
        let forum = match src.doc.get("forum") {
            Some(f) => Some(refs::forum(ctx, &w.engine, f).await?),
            None => match refs::forum_or_current(ctx, &w.engine, None).await {
                Ok(f) => Some(f),
                Err(CliError::Usage(_)) => None,
                Err(e) => return Err(e),
            },
        };
        let draft = match post_draft(&src.doc, forum.clone()) {
            Ok(d) => d,
            Err(errors) => {
                let id = keep(ctx, &drafts, &mut src, &errors)?;
                if interactive && prompt::confirm("The draft has errors. Edit it?") {
                    edit_kept(&drafts, &id)?;
                    src.doc = drafts.load(&id)?;
                    continue;
                }
                return Err(CliError::Engine(
                    EngineError::invalid_input(format!(
                        "{} (kept as draft {id}; fix it with `lessdugin draft edit {id}`)",
                        errors.join("; ")
                    ))
                    .with_detail("draft", id.as_str()),
                ));
            }
        };
        if !yes {
            if !interactive {
                let id = keep(ctx, &drafts, &mut src, &[])?;
                return Err(CliError::usage(format!(
                    "not running interactively, so nothing was published: kept as draft {id}; \
                     publish it with `lessdugin draft publish {id} --yes`"
                )));
            }
            summary(ctx, &draft, forum.as_deref().unwrap_or_default());
            match ask_publish() {
                Answer::Yes => {}
                Answer::Edit => {
                    let id = keep(ctx, &drafts, &mut src, &[])?;
                    edit_kept(&drafts, &id)?;
                    src.doc = drafts.load(&id)?;
                    continue;
                }
                Answer::No => {
                    let id = keep(ctx, &drafts, &mut src, &[])?;
                    ctx.out.note(&[(
                        cat([
                            lit("Not published; kept as draft "),
                            tt(&id),
                            lit(" (`lessdugin draft publish "),
                            tt(&id),
                            lit("`)."),
                        ]),
                        Style::Dim,
                    )]);
                    return Ok(());
                }
            }
        }
        match w.engine.publish(&draft).await {
            Ok(ticket) => {
                if let Some(id) = &src.draft {
                    drafts.remove(id)?;
                }
                if let Some(f) = &forum
                    && ctx.state.current_forum().is_none()
                {
                    let _ = ctx.state.set_current_forum(Some(f));
                }
                let t = write::drive(ctx, &w, ticket).await?;
                return write::report(ctx, &t, "Published");
            }
            Err(e) if e.code == lessdugin_core::engine::ErrorCode::InvalidInput => {
                let id = keep(ctx, &drafts, &mut src, std::slice::from_ref(&e.message))?;
                return Err(CliError::Engine(e.with_detail("draft", id.as_str())));
            }
            Err(e) => {
                if src.draft.is_some() {
                    keep(ctx, &drafts, &mut src, &[])?;
                }
                return Err(e.into());
            }
        }
    }
}

/// The `draft ls --json` entry.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftEntry {
    id: String,
    #[serde(rename = "type")]
    draft_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    forum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    modified_ms: i64,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct DraftList {
    dto: u32,
    drafts: Vec<DraftEntry>,
}

/// `lessdugin draft [ls|edit|rm|publish]`.
pub async fn run(ctx: &Ctx, args: DraftArgs) -> Result<(), CliError> {
    let drafts = Drafts::new(ctx);
    match args.action.unwrap_or(DraftAction::Ls) {
        DraftAction::Ls => {
            let mut list = Vec::new();
            for (id, ms) in drafts.list() {
                let path = drafts.path(&id).display().to_string();
                let entry = match drafts.load(&id) {
                    Ok(doc) => DraftEntry {
                        draft_type: doc.draft_type().map_or("?", DraftType::as_str),
                        forum: doc.get("forum").map(str::to_owned),
                        title: doc.get("title").map(str::to_owned).or_else(|| {
                            doc.body
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .map(|l| l.chars().take(60).collect())
                        }),
                        target: doc
                            .get("target")
                            .or_else(|| doc.get("parent"))
                            .or_else(|| doc.get("root"))
                            .map(str::to_owned),
                        id: id.clone(),
                        modified_ms: ms,
                        path,
                        error: None,
                    },
                    Err(e) => DraftEntry {
                        id: id.clone(),
                        draft_type: "?",
                        forum: None,
                        title: None,
                        target: None,
                        modified_ms: ms,
                        path,
                        error: Some(match e {
                            CliError::Engine(e) => e.message,
                            CliError::Usage(m) => m,
                            CliError::Reported(_) => String::new(),
                        }),
                    },
                };
                list.push(entry);
            }
            if ctx.out.is_json() {
                ctx.out.json(&DraftList {
                    dto: lessdugin_core::dto::DTO_VERSION,
                    drafts: list,
                });
                return Ok(());
            }
            if list.is_empty() {
                ctx.out.line(&[(lit("No drafts."), Style::Dim)]);
            }
            for d in &list {
                ctx.out.line(&[
                    (tt(&d.id), Style::Bold),
                    (lit("  "), Style::Plain),
                    (lit(d.draft_type), Style::Dim),
                    (lit("  "), Style::Plain),
                    (tt(d.title.as_deref().unwrap_or("")), Style::Plain),
                    (lit("  "), Style::Plain),
                    (tt(&render::time(d.modified_ms)), Style::Dim),
                ]);
            }
            Ok(())
        }
        DraftAction::Edit { id } => {
            drafts.load(&id).or_else(|e| match e {
                // a malformed draft can still be edited
                CliError::Engine(ref x)
                    if x.code == lessdugin_core::engine::ErrorCode::InvalidInput =>
                {
                    Ok(Doc::default())
                }
                e => Err(e),
            })?;
            edit_kept(&drafts, &id)
        }
        DraftAction::Rm { id } => {
            if !drafts.path(&id).exists() {
                check_id(&id)?;
                return Err(CliError::Engine(EngineError::not_found(format!(
                    "no draft {id}"
                ))));
            }
            drafts.remove(&id)?;
            if ctx.out.is_json() {
                ctx.out
                    .json(&serde_json::json!({"dto": 1, "id": id, "removed": true}));
            } else {
                ctx.out.line(&[
                    (lit("Removed draft "), Style::Plain),
                    (tt(&id), Style::Bold),
                ]);
            }
            Ok(())
        }
        DraftAction::Publish { id, yes } => {
            let doc = drafts.load(&id)?;
            match doc
                .draft_type()
                .map_err(|e| CliError::Engine(EngineError::invalid_input(e)))?
            {
                DraftType::Post => {
                    publish_post(
                        ctx,
                        PostSource {
                            doc,
                            draft: Some(id),
                        },
                        yes,
                    )
                    .await
                }
                DraftType::Comment => write::publish_comment_draft(ctx, &id, doc).await,
                DraftType::Edit => write::publish_edit_draft(ctx, &id, doc).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_round_trip() {
        let text = "---\ntype: post\n# a comment\nforum: \"alice.test/meta\"\ntitle: 'It''s here'\nkind: quick-take\ntags: [a, \"b c\"]\n---\nBody\n";
        let doc = Doc::parse(text).unwrap();
        assert_eq!(doc.get("title"), Some("It's here"));
        assert_eq!(doc.list("tags"), ["a", "b c"]);
        assert_eq!(doc.body, "Body\n");
        let back = Doc::parse(&doc.render(&["bad\nthing".into()])).unwrap();
        assert_eq!(back, doc);
        assert!(doc.render(&["x".into()]).starts_with("---\n# error: x\n"));
        let d = post_draft(&doc, Some("at://f".into())).unwrap();
        assert_eq!(d.kind, PostKind::QuickTake);
        assert_eq!(d.body.markdown, "Body\n");
    }

    #[test]
    fn plain_files_and_errors() {
        let doc = Doc::parse("# Title\n\ntext").unwrap();
        assert!(doc.fields.is_empty());
        assert!(Doc::parse("---\ntitle: x\n").is_err());
        assert!(Doc::parse("---\nnot a field\n---\n").is_err());
        let errs = post_draft(&Doc::parse("---\nkind: link\n---\n").unwrap(), None).unwrap_err();
        assert_eq!(errs.len(), 3, "{errs:?}");
        let errs = post_draft(
            &Doc::parse("---\nkind: poem\n---\nx").unwrap(),
            Some("f".into()),
        )
        .unwrap_err();
        assert!(errs[0].contains("poem"), "{errs:?}");
    }

    /// TERM-8: no text from the network in the first or last five lines of a draft unless it
    /// is neutralised, error lines are terminal-safe, and the tail is never part of the body.
    #[test]
    fn rendered_drafts_are_editor_safe() {
        use lessdugin_native::template::MODELINE_LINES;
        let mut doc =
            Doc::parse("---\ntitle: \"vim: set ft=sh:\"\n---\nvim: set shell=evil:\n").unwrap();
        doc.set("forum", Value::Str("ex: x".into()));
        let errors = [
            "vi: set foo=bar: -*- mode: x -*-\nLocal Variables:".to_owned(),
            "bad\u{2028}line\u{202e}gnp.exe\u{1b}[2J".to_owned(),
        ];
        let text = doc.render(&errors);
        let lines: Vec<&str> = text.lines().collect();
        for l in lines
            .iter()
            .take(MODELINE_LINES)
            .chain(lines.iter().rev().take(MODELINE_LINES))
        {
            assert!(
                *l == "---" || *l == "#" || l.starts_with("# "),
                "only fixed or neutralised lines at the ends: {l:?}"
            );
            assert_eq!(*l, neutralise_modelines(l), "{l:?}");
        }
        // two errors, two lines (a newline or U+2028 in an error does not start a line)
        assert_eq!(
            lines.iter().filter(|l| l.starts_with(ERROR_PREFIX)).count(),
            2
        );
        for c in ['\u{2028}', '\u{202e}', '\u{1b}'] {
            assert!(!text.contains(c), "{c:?} in {text:?}");
        }
        assert!(
            !lines[1].contains("vi:") && !lines[1].contains("-*-"),
            "{}",
            lines[1]
        );
        // the fields and the body sit between the fixed lines, and come back unchanged
        let back = Doc::parse(&text).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.body, "vim: set shell=evil:\n");
        // text added under the scissors line is ignored; a file without it is all body
        let edited = text.replace(SCISSORS, &format!("{SCISSORS}\nadded below"));
        assert_eq!(Doc::parse(&edited).unwrap(), doc);
        assert_eq!(Doc::parse("a\nb\n").unwrap().body, "a\nb\n");
    }

    #[test]
    fn standard_input_over_the_limit_is_refused() {
        let path = Path::new("-");
        let max = usize::try_from(DRAFT_MAX_BYTES).unwrap();
        let ok = read_limited(&vec![b'a'; max][..], path).unwrap();
        assert_eq!(ok.len(), max);
        let Err(CliError::Engine(e)) = read_limited(&vec![b'a'; max + 1][..], path) else {
            panic!("a text over the limit must be refused");
        };
        assert_eq!(e.code, lessdugin_core::engine::ErrorCode::TooLarge);
        assert!(read_limited(&[0xff, 0xfe][..], path).is_err());
    }
}
