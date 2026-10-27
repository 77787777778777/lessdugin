// SPDX-License-Identifier: AGPL-3.0-or-later
//! Media: `media add|get|open` (`synthesis-v1.md` §6.10) and the draft-local images of a post
//! (`lfm-1.md` §12). The pipeline is the core's (`prepare_media`: sniff, strip, orient, CID,
//! `docs/plan.md` M4), so the CLI and the SPA publish the same bytes.
//!
//! **Draft-local images.** When a post is published (`post`, `draft publish`), every inline
//! image whose destination is a path on this machine (`![a chart](./chart.png)`: relative to the
//! post's file, else to the working directory) is prepared, and its destination becomes
//! `cid:<cid>`. A `cid:` image whose bytes this device prepared (`media add`) is attached too.
//! The media item carries the image's markdown alt, and an image without one is warned about
//! (`data-model.md` §4). The draft keeps its paths; the bytes are uploaded when the write is sent
//! (`data-model.md` §8.8).
//!
//! **Downloads.** `media get` saves item N of a post or comment (`lfm-1.md` §6.2 numbering) as
//! `<cid>.<ext-from-sniffed-type>` (`data-model.md` §4), never under the record's `name`, which
//! the author chose; the CID is checked (`^b[a-z2-7]{1,127}$`) before it names a file. The host
//! streams the blob from the author's PDS to a file while hashing it, and fails unless the bytes
//! match the CID (`Host::fetch_blob`); the CLI copies that file and never holds the bytes.

use std::ops::Range;
use std::path::{Path, PathBuf};

use lessdugin_atp::syntax::Did;
use lessdugin_core::dto::{BodyDraft, CommentSort, MediaInput, MediaRef, ThreadOpts, ThreadRoot};
use lessdugin_core::engine::{Engine, EngineError, ErrorCode};
use lessdugin_core::host::Host;
use lessdugin_core::limits::BLOB_HARD_MAX_BYTES;
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

/// A path of a draft: `~/…` under `$HOME`, a relative path under `base`.
fn resolve(path: &str, base: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// The bytes of a media file (at most the hard blob limit, `limits.md` §5).
fn read_media(path: &Path) -> Result<Vec<u8>, CliError> {
    let meta = std::fs::metadata(path).map_err(|e| {
        let msg = format!("{}: {e}", path.display());
        if e.kind() == std::io::ErrorKind::NotFound {
            EngineError::not_found(msg)
        } else {
            EngineError::invalid_input(msg)
        }
    })?;
    if !meta.is_file() {
        return Err(EngineError::invalid_input(format!("{} is not a file", path.display())).into());
    }
    if meta.len() > BLOB_HARD_MAX_BYTES {
        return Err(EngineError::new(
            ErrorCode::TooLarge,
            format!(
                "{} is larger than {} MiB (limits.md §5)",
                path.display(),
                BLOB_HARD_MAX_BYTES / (1024 * 1024)
            ),
        )
        .into());
    }
    std::fs::read(path)
        .map_err(|e| EngineError::invalid_input(format!("{}: {e}", path.display())).into())
}

/// One media item a publish attaches (the summary lists them).
pub struct Attached {
    /// What the text named: the path, or `cid:…`.
    pub source: String,
    /// The reference in the body.
    pub media: MediaRef,
}

/// Attaches the media of `body` before it is published: each draft-local image is prepared and
/// its destination rewritten to `cid:<cid>`, and each `cid:` image whose bytes this device
/// prepared is added to `body.media` (one item per CID). Returns the attached items in the order
/// of `body.media`; an image without alt text is warned about.
pub async fn attach(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    body: &mut BodyDraft,
    base: &Path,
) -> Result<Vec<Attached>, CliError> {
    let found = images(&body.markdown).map_err(EngineError::invalid_input)?;
    let mut attached: Vec<Attached> = Vec::new();
    let mut rewrites: Vec<(Range<usize>, String)> = Vec::new();
    let mut no_alt: Vec<String> = Vec::new();
    for img in found.local {
        let bytes = read_media(&resolve(&img.path, base))?;
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
            .await?;
        rewrites.push((img.dest, format!("cid:{}", m.cid)));
        if img.alt.is_empty() {
            no_alt.push(img.path.clone());
        }
        push_media(&mut attached, &mut body.media, img.path, m);
    }
    for (cid, alt) in found.cids {
        if body.media.iter().any(|m| m.cid == cid) {
            if alt.is_empty() {
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
        };
        push_media(&mut attached, &mut body.media, format!("cid:{cid}"), m);
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
            lit(" has no alt text; describe it for readers who cannot see it: ![description](…)"),
        ]));
    }
    Ok(attached)
}

/// Adds `m` to the body's media unless its CID is there (keeping the first alt given).
fn push_media(
    attached: &mut Vec<Attached>,
    media: &mut Vec<MediaRef>,
    source: String,
    m: MediaRef,
) {
    if let Some(have) = media.iter_mut().find(|x| x.cid == m.cid) {
        if have.alt.is_none() {
            have.alt = m.alt;
        }
        return;
    }
    media.push(m.clone());
    attached.push(Attached { source, media: m });
}

// ---------------------------------------------------------------------------------------------
// media add

/// `lessdugin media add <file> [--alt]`: prepares a file on this device and prints the
/// reference to put in a post (`![alt](cid:…)`); the bytes stay here until a post that uses it
/// is published.
async fn add(ctx: &Ctx, file: &Path, alt: Option<String>) -> Result<(), CliError> {
    let engine = ctx.engine()?;
    let bytes = read_media(file)?;
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
        .await?;
    if alt.is_none() {
        ctx.out.warn(lit(
            "no alt text: describe the file for readers who cannot see it (--alt, or in the \
             ![…] of the post)",
        ));
    }
    if ctx.out.is_json() {
        ctx.out.json(&dto(&m));
        return Ok(());
    }
    ctx.out.line(&[
        (lit("Prepared "), Style::Good),
        (tt(&file.display().to_string()), Style::Bold),
        (
            cat([lit(" ("), tt(&m.mime), lit(", "), size(m.size_bytes), lit(")")]),
            Style::Dim,
        ),
    ]);
    ctx.out.line(&[(
        tt(&format!("![{}](cid:{})", alt.as_deref().unwrap_or(""), m.cid)),
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
    /// `<cid>.<ext-from-sniffed-type>`.
    fn file_name(&self) -> String {
        format!("{}.{}", self.cid, ext_of(self.sniffed.unwrap_or("")))
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
    let (author, body) = if is_comment {
        let c = thread
            .comments
            .iter()
            .find(|c| c.uri == uri)
            .ok_or_else(|| EngineError::not_found(format!("{uri} is not in its thread")))?;
        (c.author.did.clone(), &c.body)
    } else {
        match &thread.post {
            ThreadRoot::Post(p) => (p.post.author.did.clone(), &p.body),
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
    let id = engine.resolve_identity(&author).await?;
    let pds = id
        .identity
        .pds
        .as_deref()
        .and_then(|p| url::Url::parse(p).ok())
        .ok_or_else(|| {
            EngineError::not_found(format!("the DID document of {author} names no PDS"))
        })?;
    let did = Did::parse(&author).map_err(|e| EngineError::internal(e.to_string()))?;
    let h = engine
        .host()
        .fetch_blob(pds, did, m.cid.clone(), BLOB_HARD_MAX_BYTES)
        .await
        .map_err(EngineError::from)?;
    let file = PathBuf::from(&h.locator);
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

/// Copies `from` to `to` through a temporary file next to it, so `to` is never half written.
fn save(from: &Path, to: &Path) -> Result<(), CliError> {
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = to
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let part = dir.join(format!(".{name}.part"));
    std::fs::copy(from, &part)
        .and_then(|_| std::fs::rename(&part, to))
        .map_err(|e| {
            let _ = std::fs::remove_file(&part);
            EngineError::invalid_input(format!("could not write {}: {e}", to.display())).into()
        })
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

/// The warning when the declared type is not the sniffed one (`lfm-1.md` §6.2).
fn type_warning(ctx: &Ctx, f: &Fetched) {
    if f.sniffed != Some(f.declared.as_str()) {
        ctx.out.warn(cat([
            lit("the file is declared as "),
            tt(&f.declared),
            lit(" but its content is "),
            tt(f.sniffed.unwrap_or("of no known type")),
        ]));
    }
}

/// `lessdugin media get <ref> --index N [-o path]`.
async fn get(
    ctx: &Ctx,
    reference: &str,
    index: u16,
    out: Option<&Path>,
) -> Result<(), CliError> {
    let f = fetch(ctx, reference, index).await?;
    type_warning(ctx, &f);
    let to = match out {
        Some(p) if p.is_dir() => p.join(f.file_name()),
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(f.file_name()),
    };
    save(&f.file, &to)?;
    if ctx.out.is_json() {
        ctx.out.json(&Saved {
            dto: lessdugin_core::dto::DTO_VERSION,
            cid: f.cid,
            mime: f.sniffed,
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
                tt(f.sniffed.unwrap_or("unknown type")),
                lit(", "),
                size(f.size),
                lit("; the bytes match the CID)"),
            ]),
            Style::Dim,
        ),
    ]);
    Ok(())
}

/// `lessdugin media open <ref> --index N`: saves the item under the cache directory as
/// `<cid>.<ext>` and opens it with the system's viewer (`$BROWSER`, then the desktop's). Only a
/// type of `data-model.md` §8.8's table is opened.
async fn open(ctx: &Ctx, reference: &str, index: u16) -> Result<(), CliError> {
    let f = fetch(ctx, reference, index).await?;
    type_warning(ctx, &f);
    if f.sniffed.is_none() {
        return Err(EngineError::invalid_input(format!(
            "media item {index} is of no type this client opens; save it with `lessdugin media \
             get {reference} --index {index}`"
        ))
        .into());
    }
    let dir = ctx.paths.cache_dir.join("open");
    std::fs::create_dir_all(&dir)
        .map_err(|e| EngineError::internal(format!("{}: {e}", dir.display())))?;
    let to = dir.join(f.file_name());
    save(&f.file, &to)?;
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
        (
            lit(if opened { "Opened " } else { "Saved " }),
            Style::Good,
        ),
        (tt(&target), Style::Bold),
    ]);
    if !opened {
        ctx.out.warn(lit("no viewer could be started; open the file yourself"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_destinations() {
        for p in ["./chart.png", "chart.png", "../img/a.jpg", "/abs/a.png", "~/a.png"] {
            assert!(is_local(p), "{p}");
        }
        // a Windows drive letter is a path; a scheme is not
        assert!(is_local("C:\\img\\a.png"));
        for u in ["https://example.com/a.png", "cid:bafkrei", "mailto:a@b", "#x", "//h/a.png"] {
            assert!(!is_local(u), "{u}");
        }
    }

    #[test]
    fn cids_name_files_only_when_valid() {
        assert!(valid_cid("bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"));
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
}
