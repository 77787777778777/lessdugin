// SPDX-License-Identifier: AGPL-3.0-or-later
//! Media: `media add|get|open` (`synthesis-v1.md` §6.10) and the draft-local images of a post
//! (`lfm-1.md` §12). The pipeline is the core's (`prepare_media`: sniff, strip, orient, CID,
//! `docs/plan.md` M4), so the CLI and the SPA publish the same bytes.
//!
//! **Draft-local images.** When a post is published (`post`, `draft publish`), and when a
//! comment or an edit is written, every inline image whose destination is a path on this
//! machine (`![a chart](img/chart.png)`) is prepared, and its destination becomes `cid:<cid>`.
//! A `cid:` image whose bytes this device prepared (`media add`) is attached too. The media item
//! carries the image's markdown alt, and an image without one is warned about (`data-model.md`
//! §4). An edit keeps the item's media. The draft keeps its paths; the bytes are uploaded when
//! the write is sent (`data-model.md` §8.8). The path is the draft's text, which may come from
//! anyone (`threat-model.md` F3), so FILE-1 holds: it is relative, without `~` or `..`, lies
//! inside the canonical directory that contains the draft (the text's file's, or
//! `$DATA/drafts/`) after canonicalisation, and names a regular file, read with `O_NOFOLLOW` and
//! checked with `fstat`; anything else is an error naming the reference. The publish summary
//! lists each file with its absolute path, size and sniffed type (FILE-2, in `draft.rs`); a
//! comment or an edit lists them and asks ([`confirm_uploads`]).
//!
//! **Downloads.** `media get` saves item N of a post or comment (`lfm-1.md` §6.2 numbering) as
//! `<cid>.<ext>` (`data-model.md` §4, FILE-3): the extension comes from the sniffed type when the
//! declared type agrees with it, else `.bin`; never from the record's `name`, which the author
//! chose; and the CID is checked (`^b[a-z2-7]{1,127}$`) before it names a file. Without `-o` the
//! file goes to the working directory and never replaces one (`O_EXCL`). The engine's
//! `load_media` fetches the blob from the author's PDS (from their DID document), at most its
//! declared size, and the host streams it to a file while hashing it, failing unless the bytes
//! match the CID (`Host::fetch_blob`); the CLI copies that file and never holds the bytes.
//! `media open` opens only images, video and audio (FILE-4), handing the path to the opener as
//! one argument.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use lessdugin_core::dto::{
    BodyDraft, CommentSort, MediaInput, MediaRef, NoticeCode, ThreadOpts, ThreadRoot,
};
use lessdugin_core::engine::{Engine, EngineError, ErrorCode};
use lessdugin_core::limits::BLOB_HARD_MAX_BYTES;
use lessdugin_core::media::MediaOwner;
use lessdugin_core::outbox::blob::normalise_mime;
use lessdugin_native::host::NativeHost;
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};
use serde::Serialize;

use crate::args::{MediaAction, MediaArgs};
use crate::cmd::{read, refs};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, cat, dto, lit, tt};

/// `lessdugin media add|get|open`.
pub async fn run(ctx: &Ctx, args: MediaArgs) -> Result<(), CliError> {
    match args.action {
        MediaAction::Add { file, alt } => add(ctx, &file, alt).await,
        MediaAction::Get {
            reference,
            index,
            out,
        } => get(ctx, &reference, index, out.as_deref()).await,
        MediaAction::Open { reference, index } => open(ctx, &reference, index).await,
    }
}

// ---------------------------------------------------------------------------------------------
// Draft-local images

/// An inline image whose destination is a file on this machine.
#[derive(Debug, PartialEq, Eq)]
struct LocalImage {
    /// Where the destination is written in the source.
    dest: Range<usize>,
    /// The destination.
    path: String,
    /// The markdown alt, trimmed.
    alt: String,
}

/// The images of a body that publishing attaches.
#[derive(Debug, Default, PartialEq, Eq)]
struct Images {
    /// Draft-local files.
    local: Vec<LocalImage>,
    /// `cid:` images: the CID and the markdown alt.
    cids: Vec<(String, String)>,
}

/// True for a CID that may name a file (`lfm-1.md` §6.1: `^b[a-z2-7]{1,127}$`).
fn valid_cid(c: &str) -> bool {
    c.len() >= 2
        && c.len() <= 128
        && c.starts_with('b')
        && c[1..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// The CID of a `cid:` destination (`cid:` matched ASCII case-insensitively, `lfm-1.md` §6.1).
fn cid_of(dest: &str) -> Option<&str> {
    let c = dest
        .get(..4)
        .filter(|p| p.eq_ignore_ascii_case("cid:"))
        .map(|_| &dest[4..])?;
    valid_cid(c).then_some(c)
}

/// True for a destination naming a file here: no URL scheme (a scheme has two characters or
/// more, so a Windows drive letter is a path), not protocol-relative, not a fragment.
fn is_local(dest: &str) -> bool {
    if dest.trim().is_empty() || dest.starts_with('#') || dest.starts_with("//") {
        return false;
    }
    match dest.split_once(':') {
        Some((scheme, _)) => {
            !(scheme.len() >= 2
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
        }
        None => true,
    }
}

/// Where `dest` is written in the source `img` of an inline image (`![…](dest "title")`): after
/// the rightmost `](` that is followed, past blanks, by `dest` (plain, then a blank or `)`; or in
/// `<…>`).
fn destination_in(img: &str, dest: &str) -> Option<Range<usize>> {
    let blank = [' ', '\t', '\n', '\r'];
    let mut end = img.len();
    while let Some(i) = img[..end].rfind("](") {
        let after = &img[i + 2..];
        let at = i + 2 + (after.len() - after.trim_start_matches(blank).len());
        let rest = &img[at..];
        if let Some(inner) = rest.strip_prefix('<')
            && inner.starts_with(dest)
            && inner[dest.len()..].starts_with('>')
        {
            return Some(at + 1..at + 1 + dest.len());
        }
        if rest.starts_with(dest) && rest[dest.len()..].starts_with([' ', '\t', '\n', '\r', ')']) {
            return Some(at..at + dest.len());
        }
        end = i;
    }
    None
}

/// The images of `src` that publishing attaches, parsed as `lfm/1` parses (`lessdugin_markdown`'s
/// options), so an image in a code block is text. An image inside another image's alt is never
/// shown (`lfm-1.md` §6.3) and is skipped. A draft-local file must be an inline image: the
/// destination of a reference-style image lives in its definition, which this does not rewrite.
fn images(src: &str) -> Result<Images, String> {
    let mut out = Images::default();
    // the outermost image being read: (destination, link type, source range, alt)
    let mut open: Option<(String, LinkType, Range<usize>, String)> = None;
    let mut depth = 0usize;
    for (ev, range) in Parser::new_ext(src, lessdugin_markdown::OPTIONS).into_offset_iter() {
        match ev {
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                depth += 1;
                if depth == 1 {
                    open = Some((dest_url.into_string(), link_type, range, String::new()));
                }
            }
            Event::End(TagEnd::Image) => {
                depth = depth.saturating_sub(1);
                if depth > 0 {
                    continue;
                }
                let Some((dest, link_type, range, alt)) = open.take() else {
                    continue;
                };
                let alt = alt.trim().to_owned();
                if let Some(c) = cid_of(&dest) {
                    out.cids.push((c.to_owned(), alt));
                    continue;
                }
                if !is_local(&dest) {
                    continue;
                }
                if link_type != LinkType::Inline {
                    return Err(format!(
                        "the image {dest:?} is a reference-style image; write a file on this \
                         machine as ![alt]({dest})"
                    ));
                }
                let Some(r) = destination_in(&src[range.clone()], &dest) else {
                    return Err(format!(
                        "the image path {dest:?} must be written as it is, without escapes \
                         (![alt](path) or ![alt](<path with spaces>))"
                    ));
                };
                out.local.push(LocalImage {
                    dest: range.start + r.start..range.start + r.end,
                    path: dest,
                    alt,
                });
            }
            Event::Text(t) | Event::Code(t) if depth > 0 => {
                if let Some(o) = open.as_mut() {
                    o.3.push_str(&t);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// True for `C:…`, an absolute Windows path on any platform.
fn drive_letter(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// The file a draft's image path names (`threat-model.md` FILE-1): `path` is relative, without
/// `~` or `..`, and after canonicalisation lies inside `dir`, the canonical directory that
/// contains the draft. The final component is not a symlink (the file is then read with
/// `O_NOFOLLOW`). The error names the reference.
fn draft_file(path: &str, dir: &Path) -> Result<PathBuf, EngineError> {
    let bad = |why: String| EngineError::invalid_input(format!("the image path {path:?} {why}"));
    let p = Path::new(path);
    if p.has_root() || drive_letter(path) || path.starts_with(['/', '\\']) {
        return Err(bad("must be relative to the draft's directory".into()));
    }
    if path.starts_with('~') {
        return Err(bad(
            "must be relative to the draft's directory, not to ~".into()
        ));
    }
    if p.components().any(|c| c == Component::ParentDir)
        || path.split(['/', '\\']).any(|c| c == "..")
    {
        return Err(bad("must not leave the draft's directory (..)".into()));
    }
    let joined = dir.join(p);
    match std::fs::symlink_metadata(&joined) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(bad("is a symbolic link; name the file itself".into()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(EngineError::not_found(format!(
                "the image path {path:?} names no file ({})",
                joined.display()
            )));
        }
        Err(e) => return Err(bad(format!("cannot be read: {e}"))),
    }
    let canon = joined
        .canonicalize()
        .map_err(|e| bad(format!("cannot be read: {e}")))?;
    if !canon.starts_with(dir) {
        return Err(bad(format!(
            "leads outside the draft's directory {}",
            dir.display()
        )));
    }
    Ok(canon)
}

/// The bytes of a media file the user named: a regular file (`fstat`), not a symlink
/// (`O_NOFOLLOW`), of at most the hard blob limit (`limits.md` §5). `what` names it in errors.
fn read_media(path: &Path, what: &str) -> Result<Vec<u8>, EngineError> {
    lessdugin_native::fsutil::read_regular_nofollow(path, BLOB_HARD_MAX_BYTES).map_err(|e| match e
        .kind()
    {
        std::io::ErrorKind::NotFound => {
            EngineError::not_found(format!("{what}: {} does not exist", path.display()))
        }
        std::io::ErrorKind::InvalidData => EngineError::new(
            ErrorCode::TooLarge,
            format!(
                "{what} is larger than {} MiB (limits.md §5)",
                BLOB_HARD_MAX_BYTES / (1024 * 1024)
            ),
        ),
        _ => EngineError::invalid_input(format!("{what}: {e}")),
    })
}

/// Names `what` in an error of `prepare_media` (an unsupported type, a container too deep).
fn naming(what: &str, mut e: EngineError) -> EngineError {
    e.message = format!("{what}: {}", e.message);
    e
}

/// PRIV-7: only images and MP4 are stripped; other files carry their metadata.
fn metadata_warning(ctx: &Ctx, what: &str, mime: &str) {
    if !(mime.starts_with("image/") || mime == "video/mp4" || mime == "audio/mp4") {
        ctx.out.warn(cat([
            tt(what),
            lit(" ("),
            tt(mime),
            lit(
                ") is published as it is: metadata in it (such as PDF author fields or audio \
                 tags) is not removed",
            ),
        ]));
    }
}

/// One media item a publish attaches (the summary lists them).
pub struct Attached {
    /// What the text named: the path, or `cid:…`.
    pub source: String,
    /// The file of a draft-local image (canonical); `None` for a `cid:` image prepared here.
    pub file: Option<PathBuf>,
    /// The reference in the body.
    pub media: MediaRef,
}

/// Attaches the media of `body` before it is published: each draft-local image is prepared and
/// its destination rewritten to `cid:<cid>`, and each `cid:` image whose bytes this device
/// prepared is added to `body.media` (one item per CID). `dir` is the directory that contains
/// the draft; `None` (standard input) has none, so a draft-local image is an error. Returns the
/// attached items in the order of `body.media`; an image without alt text is warned about.
pub async fn attach(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    body: &mut BodyDraft,
    dir: Option<&Path>,
) -> Result<Vec<Attached>, CliError> {
    let found = images(&body.markdown).map_err(EngineError::invalid_input)?;
    let mut attached: Vec<Attached> = Vec::new();
    let mut rewrites: Vec<(Range<usize>, String)> = Vec::new();
    let mut no_alt: Vec<String> = Vec::new();
    let canon_dir = match (dir, found.local.first()) {
        (_, None) => None,
        (None, Some(img)) => {
            return Err(EngineError::invalid_input(format!(
                "the image path {:?} names a file, but a text from -m or standard input has no \
                 directory; prepare the file with `lessdugin media add` and name it by its cid:",
                img.path
            ))
            .into());
        }
        (Some(d), Some(_)) => Some(d.canonicalize().map_err(|e| {
            EngineError::invalid_input(format!("the draft's directory {}: {e}", d.display()))
        })?),
    };
    for img in found.local {
        let Some(dir) = canon_dir.as_deref() else {
            break;
        };
        let file = draft_file(&img.path, dir)?;
        let what = format!("the image {:?}", img.path);
        let bytes = read_media(&file, &what)?;
        let alt = (!img.alt.is_empty()).then(|| img.alt.clone());
        let m = engine
            .prepare_media(
                bytes,
                &MediaInput {
                    declared_mime: None,
                    alt,
                    name: None,
                    aspect_ratio: None,
                },
            )
            .await
            .map_err(|e| naming(&what, e))?;
        rewrites.push((img.dest, format!("cid:{}", m.cid)));
        // the core says when a file has no alt text (`data-model.md` §4)
        if m.notices.iter().any(|n| n.code == NoticeCode::MissingAlt) {
            no_alt.push(img.path.clone());
        }
        if !body.media.iter().any(|x| x.cid == m.cid) {
            metadata_warning(ctx, &img.path, &m.mime);
        }
        push_media(&mut attached, &mut body.media, img.path, Some(file), m);
    }
    for (cid, alt) in found.cids {
        // an item the body has (attached above, or kept by an edit): its own alt describes it
        // when the markdown has none (`lfm-1.md` §6.2)
        if let Some(have) = body.media.iter().find(|m| m.cid == cid) {
            if alt.is_empty() && have.alt.is_none() {
                no_alt.push(format!("cid:{cid}"));
            }
            continue;
        }
        // a reference whose bytes are not on this device stays as written (it renders as
        // missing media, `lfm-1.md` §6.1)
        let Some(b) = engine.prepared_media(&cid).await? else {
            continue;
        };
        if alt.is_empty() {
            no_alt.push(format!("cid:{cid}"));
        }
        let m = MediaRef {
            id: cid.clone(),
            cid: cid.clone(),
            mime: b.mime,
            size_bytes: b.bytes.len() as u64,
            alt: (!alt.is_empty()).then_some(alt),
            aspect_ratio: None,
            duration_ms: None,
            name: None,
            notices: Vec::new(),
        };
        push_media(
            &mut attached,
            &mut body.media,
            format!("cid:{cid}"),
            None,
            m,
        );
    }
    rewrites.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    for (r, s) in rewrites {
        body.markdown.replace_range(r, &s);
    }
    no_alt.dedup();
    for p in &no_alt {
        ctx.out.warn(cat([
            lit("the image "),
            tt(p),
            lit(" has no alt text; describe it for readers who cannot see it: ![description](...)"),
        ]));
    }
    Ok(attached)
}

/// Adds `m` to the body's media unless its CID is there (keeping the first alt given).
fn push_media(
    attached: &mut Vec<Attached>,
    media: &mut Vec<MediaRef>,
    source: String,
    file: Option<PathBuf>,
    m: MediaRef,
) {
    if let Some(have) = media.iter_mut().find(|x| x.cid == m.cid) {
        if have.alt.is_none() {
            have.alt.clone_from(&m.alt);
            if let Some(a) = attached.iter_mut().find(|a| a.media.cid == m.cid) {
                a.media.alt = m.alt;
            }
        }
        return;
    }
    let m = MediaRef {
        notices: Vec::new(),
        ..m
    };
    media.push(m.clone());
    attached.push(Attached {
        source,
        file,
        media: m,
    });
}

/// The files of this machine that `attached` uploads, by absolute path.
pub fn local_files(attached: &[Attached]) -> Vec<String> {
    attached
        .iter()
        .filter_map(|a| Some(a.file.as_ref()?.display().to_string()))
        .collect()
}

/// FILE-2 for a write without a publish summary (a comment, an edit): the files of this machine
/// it uploads are listed with their absolute path, type and size, and confirmed on a terminal.
/// `yes` (`comment --yes`, `edit --yes`, `draft publish --yes`) confirms them for a file of the
/// drafts directory, never for a foreign one; headless use otherwise goes through `media add`
/// and a `cid:` reference.
pub fn confirm_uploads(
    ctx: &Ctx,
    attached: &[Attached],
    foreign: bool,
    yes: bool,
) -> Result<(), CliError> {
    let local = local_files(attached);
    if local.is_empty() || (yes && !foreign) {
        return Ok(());
    }
    if !crate::prompt::stdin_is_tty() || ctx.out.is_json() {
        return Err(CliError::usage(format!(
            "this would upload files of this machine ({}); that is confirmed on a terminal{}: \
             write it there, or prepare the files with `lessdugin media add` and name them by \
             cid:",
            local.join(", "),
            if foreign {
                ", even with --yes, for a file outside the drafts directory"
            } else {
                " (or with --yes, for a file of the drafts directory)"
            }
        )));
    }
    ctx.out.err_line(&[(lit("Uploads:"), Style::Bold)]);
    for a in attached {
        let Some(f) = &a.file else { continue };
        ctx.out.err_line(&[
            (lit("  media  "), Style::Dim),
            (tt(&f.display().to_string()), Style::Plain),
            (
                cat([
                    lit(" ("),
                    tt(&a.media.mime),
                    lit(", "),
                    size(a.media.size_bytes),
                    lit(")"),
                ]),
                Style::Dim,
            ),
        ]);
    }
    if crate::prompt::confirm("Upload these files?") {
        Ok(())
    } else {
        Err(CliError::Reported(1))
    }
}

// ---------------------------------------------------------------------------------------------
// media add

/// `lessdugin media add <file> [--alt]`: prepares a file on this device and prints the
/// reference to put in a post (`![alt](cid:…)`); the bytes stay here until a post that uses it
/// is published.
async fn add(ctx: &Ctx, file: &Path, alt: Option<String>) -> Result<(), CliError> {
    let engine = ctx.engine()?;
    let what = file.display().to_string();
    let bytes = read_media(file, &what)?;
    let alt = alt.map(|a| a.trim().to_owned()).filter(|a| !a.is_empty());
    let m = engine
        .prepare_media(
            bytes,
            &MediaInput {
                declared_mime: None,
                alt: alt.clone(),
                name: None,
                aspect_ratio: None,
            },
        )
        .await
        .map_err(|e| naming(&what, e))?;
    metadata_warning(ctx, &what, &m.mime);
    for n in &m.notices {
        ctx.out.warn(cat([
            tt(&n.message),
            lit(match n.code {
                NoticeCode::MissingAlt => " (describe it with --alt, or in the ![...] of the post)",
                _ => "",
            }),
        ]));
    }
    if ctx.out.is_json() {
        ctx.out.json(&dto(&m));
        return Ok(());
    }
    ctx.out.line(&[
        (lit("Prepared "), Style::Good),
        (tt(&file.display().to_string()), Style::Bold),
        (
            cat([
                lit(" ("),
                tt(&m.mime),
                lit(", "),
                size(m.size_bytes),
                lit(")"),
            ]),
            Style::Dim,
        ),
    ]);
    ctx.out.line(&[(
        tt(&format!(
            "![{}](cid:{})",
            alt.as_deref().unwrap_or(""),
            m.cid
        )),
        Style::Plain,
    )]);
    ctx.out.note(&[(
        lit(
            "Put that line in a post; the file is uploaded when the post is published from this \
             device.",
        ),
        Style::Dim,
    )]);
    Ok(())
}

/// A byte count for people.
pub fn size(n: u64) -> crate::out::TermText {
    tt(&if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    })
}

// ---------------------------------------------------------------------------------------------
// media get / open

/// The file extension of a sniffed type (`data-model.md` §8.8's table).
fn ext_of(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "audio/mp4" => "m4a",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "application/pdf" => "pdf",
        "text/markdown" => "md",
        _ => "bin",
    }
}

/// A fetched media item: its CID, the declared and the sniffed type, its size and the host's
/// file of it.
struct Fetched {
    cid: String,
    declared: String,
    sniffed: Option<&'static str>,
    size: u64,
    file: PathBuf,
}

impl Fetched {
    /// The sniffed type, when the declared type agrees with it (`sync.md` §11.1: both go
    /// through `data-model.md` §8.8's table, which also names `audio/x-m4a` and
    /// `application/ogg`).
    fn agreed(&self) -> Option<&'static str> {
        let declared = match self.declared.as_str() {
            "audio/x-m4a" => "audio/mp4",
            "application/ogg" => "audio/ogg",
            d => d,
        };
        self.sniffed.filter(|s| *s == declared)
    }

    /// `<cid>.<ext>`: the extension of the agreed type, else `.bin` (FILE-3).
    fn file_name(&self) -> String {
        format!("{}.{}", self.cid, ext_of(self.agreed().unwrap_or("")))
    }
}

/// Fetches item `index` (1-based) of the post or comment `reference` from its author's PDS.
async fn fetch(ctx: &Ctx, reference: &str, index: u16) -> Result<Fetched, CliError> {
    let engine = ctx.reader().await?;
    let (uri, is_comment) = refs::post_or_comment(ctx, &engine, reference).await?;
    read::sync_thread_quietly(ctx, &engine, &uri).await;
    let thread = engine
        .get_thread(
            &uri,
            &ThreadOpts {
                sort: CommentSort::Old,
                unread_only: false,
            },
        )
        .await?;
    let body = if is_comment {
        let c = thread
            .comments
            .iter()
            .find(|c| c.uri == uri)
            .ok_or_else(|| EngineError::not_found(format!("{uri} is not in its thread")))?;
        &c.body
    } else {
        match &thread.post {
            ThreadRoot::Post(p) => &p.body,
            ThreadRoot::Missing { missing } => {
                return Err(EngineError::not_found(format!("{missing} is missing")).into());
            }
        }
    };
    let m = index
        .checked_sub(1)
        .and_then(|i| body.media.get(usize::from(i)))
        .ok_or_else(|| {
            EngineError::not_found(match body.media.len() {
                0 => format!("{reference} has no media"),
                n => format!("{reference} has {n} media items (1 to {n}), not {index}"),
            })
        })?;
    // the CID names the saved file, so it must be one (`lfm-1.md` §6.1)
    if !valid_cid(&m.cid) {
        return Err(EngineError::invalid_input(format!(
            "media item {index} of {reference} has no valid CID"
        ))
        .into());
    }
    // from the author's PDS (their DID document's), capped at the declared size
    let h = engine
        .load_media(&m.cid, Some(MediaOwner::of_view(m)))
        .await?;
    let file = PathBuf::from(&h.url);
    let mut head = Vec::with_capacity(64);
    {
        use std::io::Read;
        std::fs::File::open(&file)
            .and_then(|f| f.take(64).read_to_end(&mut head))
            .map_err(|e| EngineError::internal(format!("{}: {e}", file.display())))?;
    }
    Ok(Fetched {
        cid: h.cid,
        declared: m.mime.clone(),
        sniffed: normalise_mime(&head),
        size: h.size,
        file,
    })
}

/// Copies `from` to a new file `to` (`O_EXCL`: an existing file is never replaced, FILE-3), or,
/// with `replace` (a path the user named with `-o`), through a temporary file renamed over it.
/// A copy that fails leaves nothing behind.
fn save(from: &Path, to: &Path, replace: bool) -> Result<(), CliError> {
    let fail = |e: std::io::Error| -> CliError {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            EngineError::conflict(format!(
                "{} exists; it is not replaced (name another file with -o)",
                to.display()
            ))
            .into()
        } else {
            EngineError::invalid_input(format!("could not write {}: {e}", to.display())).into()
        }
    };
    let target = if replace {
        lessdugin_native::fsutil::temp_path_for(to)
    } else {
        to.to_path_buf()
    };
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(fail)?;
    let copied = std::fs::File::open(from)
        .and_then(|mut f| std::io::copy(&mut f, &mut out))
        .and_then(|_| out.sync_all());
    drop(out);
    let done = copied.and_then(|()| {
        if replace {
            std::fs::rename(&target, to)
        } else {
            Ok(())
        }
    });
    if let Err(e) = done {
        let _ = std::fs::remove_file(&target);
        return Err(fail(e));
    }
    Ok(())
}

/// `media get --json`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    dto: u32,
    cid: String,
    mime: Option<&'static str>,
    size_bytes: u64,
    path: String,
}

/// The warning when the declared type is not the sniffed one (`sync.md` §11.1).
fn type_warning(ctx: &Ctx, f: &Fetched) {
    if f.agreed().is_none() {
        ctx.out.warn(cat([
            lit("the file is declared as "),
            tt(&f.declared),
            lit(" but its content is "),
            tt(f.sniffed.unwrap_or("of no known type")),
        ]));
    }
}

/// `lessdugin media get <ref> --index N [-o path]`.
async fn get(ctx: &Ctx, reference: &str, index: u16, out: Option<&Path>) -> Result<(), CliError> {
    let f = fetch(ctx, reference, index).await?;
    type_warning(ctx, &f);
    let (to, replace) = match out {
        Some(p) if p.is_dir() => (p.join(f.file_name()), false),
        Some(p) => (p.to_path_buf(), true),
        None => (PathBuf::from(f.file_name()), false),
    };
    save(&f.file, &to, replace)?;
    if ctx.out.is_json() {
        ctx.out.json(&Saved {
            dto: lessdugin_core::dto::DTO_VERSION,
            mime: f.agreed(),
            cid: f.cid,
            size_bytes: f.size,
            path: to.display().to_string(),
        });
        return Ok(());
    }
    ctx.out.line(&[
        (lit("Saved "), Style::Good),
        (tt(&to.display().to_string()), Style::Bold),
        (
            cat([
                lit(" ("),
                tt(f.agreed().unwrap_or("unknown type")),
                lit(", "),
                size(f.size),
                lit("; the bytes match the CID)"),
            ]),
            Style::Dim,
        ),
    ]);
    Ok(())
}

/// `lessdugin media open <ref> --index N`: saves an image, video or audio item under the cache
/// directory as `<cid>.<ext>` and opens it with the system's viewer (`$BROWSER`, then the
/// desktop's opener), the path being one argument of its command line (FILE-4, FILE-5).
/// Anything else, a PDF included, is save-only (`media get`).
async fn open(ctx: &Ctx, reference: &str, index: u16) -> Result<(), CliError> {
    let f = fetch(ctx, reference, index).await?;
    type_warning(ctx, &f);
    if !f.agreed().is_some_and(|t| {
        t.starts_with("image/") || t.starts_with("video/") || t.starts_with("audio/")
    }) {
        return Err(EngineError::invalid_input(format!(
            "media item {index} is not an image, video or audio file, so it is not opened; save \
             it with `lessdugin media get {reference} --index {index}`"
        ))
        .into());
    }
    let dir = ctx.paths.cache_dir.join("open");
    lessdugin_native::fsutil::ensure_private_dir(&dir)
        .map_err(|e| EngineError::internal(format!("{}: {e}", dir.display())))?;
    let to = dir.join(f.file_name());
    save(&f.file, &to, true)?;
    // Windows: the Mark-of-the-Web, so the opener treats it as downloaded (FILE-4)
    #[cfg(windows)]
    {
        let _ = std::fs::write(
            format!("{}:Zone.Identifier", to.display()),
            "[ZoneTransfer]\r\nZoneId=3\r\n",
        );
    }
    let target = to.display().to_string();
    let t = target.clone();
    let opened = tokio::task::spawn_blocking(move || webbrowser::open(&t).is_ok())
        .await
        .unwrap_or(false);
    if ctx.out.is_json() {
        ctx.out.json(&serde_json::json!({
            "dto": lessdugin_core::dto::DTO_VERSION,
            "cid": f.cid,
            "path": target,
            "opened": opened,
        }));
        return Ok(());
    }
    ctx.out.line(&[
        (lit(if opened { "Opened " } else { "Saved " }), Style::Good),
        (tt(&target), Style::Bold),
    ]);
    if !opened {
        ctx.out
            .warn(lit("no viewer could be started; open the file yourself"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_destinations() {
        for p in [
            "./chart.png",
            "chart.png",
            "../img/a.jpg",
            "/abs/a.png",
            "~/a.png",
        ] {
            assert!(is_local(p), "{p}");
        }
        // a Windows drive letter is a path; a scheme is not
        assert!(is_local("C:\\img\\a.png"));
        for u in [
            "https://example.com/a.png",
            "cid:bafkrei",
            "mailto:a@b",
            "#x",
            "//h/a.png",
        ] {
            assert!(!is_local(u), "{u}");
        }
    }

    #[test]
    fn cids_name_files_only_when_valid() {
        assert!(valid_cid(
            "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
        ));
        for bad in ["", "b", "../x", "bafy/../../x", "Bafk", "b1", "bafk.png"] {
            assert!(!valid_cid(bad), "{bad}");
        }
        assert_eq!(cid_of("CID:bafkrei"), Some("bafkrei"));
        assert_eq!(cid_of("cid:../etc"), None);
        assert_eq!(ext_of("image/png"), "png");
        assert_eq!(ext_of("text/html"), "bin");
    }

    #[test]
    fn images_found_and_placed() {
        let src = "Intro ![a chart](./chart.png \"t\")\n\n\
                   `![code](./no.png)`\n\n\
                   ![](<my pic.jpg>) and ![x](https://example.com/a.png) \
                   and ![y](cid:bafkreiabc)\n\n\
                   ```\n![fenced](./no2.png)\n```\n";
        let f = images(src).unwrap();
        let paths: Vec<_> = f.local.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["./chart.png", "my pic.jpg"]);
        assert_eq!(&src[f.local[0].dest.clone()], "./chart.png");
        assert_eq!(&src[f.local[1].dest.clone()], "my pic.jpg");
        assert_eq!(f.local[0].alt, "a chart");
        assert_eq!(f.local[1].alt, "");
        assert_eq!(f.cids, [("bafkreiabc".to_owned(), "y".to_owned())]);
        // an image in another image's alt is never shown, so it is not attached
        let f = images("![outer ![inner](./in.png)](./out.png)").unwrap();
        let paths: Vec<_> = f.local.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["./out.png"]);
        // a reference-style local image cannot be rewritten in place
        assert!(images("![a][r]\n\n[r]: ./a.png\n").is_err());
    }

    /// `threat-model.md` FILE-1.
    #[cfg(unix)]
    #[test]
    fn draft_paths_stay_in_the_draft_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("draft");
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img/a.png"), b"x").unwrap();
        std::fs::write(root.path().join("secret.png"), b"x").unwrap();
        std::os::unix::fs::symlink(root.path(), dir.join("up")).unwrap();
        std::os::unix::fs::symlink(dir.join("img/a.png"), dir.join("link.png")).unwrap();
        let dir = dir.canonicalize().unwrap();
        assert_eq!(
            draft_file("img/a.png", &dir).unwrap(),
            dir.join("img/a.png")
        );
        assert!(draft_file("./img/a.png", &dir).is_ok());
        for bad in [
            "/etc/passwd",
            "~/a.png",
            "~",
            "../secret.png",
            "img/../../secret.png",
            "C:\\a.png",
            "\\\\host\\a.png",
            // a directory symlink that leads out of the draft's directory
            "up/secret.png",
            // a symlink as the final component
            "link.png",
        ] {
            let e = draft_file(bad, &dir).unwrap_err();
            assert!(
                e.message.contains(&format!("{bad:?}")),
                "{bad}: {}",
                e.message
            );
        }
        assert_eq!(
            draft_file("img/none.png", &dir).unwrap_err().code,
            ErrorCode::NotFound
        );
    }
}
