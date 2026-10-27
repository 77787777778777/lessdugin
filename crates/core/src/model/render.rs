// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rendering bodies through `lessdugin_markdown` into [`RenderedBody`] DTOs, with the in-memory
//! LRU keyed by `(cid, RENDERER_VERSION)` (`sync.md` §13, `lfm-1.md` §1.2).
//!
//! A body is an `NS.markdown` union value (`data-model.md` §5): inline `text`, or a `textBlob`.
//! A spilled body is rendered from its fetched source ([`RenderCache::put_spilled`], fetched by
//! the engine from the author's PDS, `lfm-1.md` §1.3); until it is there it is **pending**: a
//! notice with the record's `excerpt` as text, never cached. An unknown union member renders as
//! a notice.
//!
//! The cached value does not depend on who renders it: the author and PDS of media views, and
//! their autoload verdict (`sync.md` §11.3), are filled in per call ([`RenderCache::body`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lessdugin_markdown::{MediaItem, RENDERER_VERSION, RenderOpts, Rendered};
use serde_json::Value;

use crate::dto::{AspectRatio, MediaView, RenderedBody, TocEntry};
use crate::media::policy::{self, Autoload, MediaCandidate};

/// The presentation version as written in DTOs.
pub fn renderer_version() -> String {
    RENDERER_VERSION.to_string()
}

/// Notice for a spilled body that is not fetched yet (pending, `lfm-1.md` §1.3).
pub const NOTICE_SPILLED: &str = "[the body is stored as a blob and is not loaded yet]";
/// Notice for a spilled body whose fetched bytes do not match its CID (`data-model.md` §5.2
/// rule 3: an "unreadable body" placeholder; the record stays valid).
pub const NOTICE_UNVERIFIED_BODY: &str = "[unreadable body: it does not match its CID]";
/// Notice for a body of an unknown union member.
pub const NOTICE_UNKNOWN_BODY: &str = "[unsupported body format]";
/// Notice for flattened structure (`lfm-1.md` §8.4).
pub const NOTICE_DEPTH: &str = "[some nested content was flattened]";
/// Notice for an unknown `profile` (rendered as `lfm/1`).
pub const NOTICE_PROFILE: &str = "[unknown markdown profile: rendered as lfm/1]";

/// An empty body (tombstones, placeholders).
pub fn empty_body() -> RenderedBody {
    RenderedBody {
        html: String::new(),
        text: String::new(),
        toc: Vec::new(),
        word_count: 0,
        read_minutes: 0,
        media: Vec::new(),
        excerpt: String::new(),
        notices: Vec::new(),
        renderer_version: renderer_version(),
    }
}

fn notices(r: &Rendered) -> Vec<String> {
    let mut n = Vec::new();
    if let Some(e) = r.error {
        n.push(e.notice().to_owned());
    }
    if r.flags.truncated {
        n.push(lessdugin_markdown::html::fixed::TRUNCATED.to_owned());
    }
    if r.flags.depth_limited {
        n.push(NOTICE_DEPTH.to_owned());
    }
    if r.flags.unknown_profile {
        n.push(NOTICE_PROFILE.to_owned());
    }
    if r.flags.math_errors > 0 {
        n.push(format!(
            "[{} math expression{} could not be rendered]",
            r.flags.math_errors,
            if r.flags.math_errors == 1 { "" } else { "s" }
        ));
    }
    n
}

fn media_views(media: &Value) -> Vec<MediaView> {
    let Some(items) = media.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let blob = m.get("blob");
            let ar = m.get("aspectRatio").and_then(|a| {
                Some(AspectRatio {
                    width: u32::try_from(a.get("width")?.as_u64()?).ok()?,
                    height: u32::try_from(a.get("height")?.as_u64()?).ok()?,
                })
            });
            MediaView {
                index: u16::try_from(i).unwrap_or(u16::MAX),
                cid: blob
                    .and_then(|b| b.get("ref"))
                    .and_then(|r| r.get("$link"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                mime: blob
                    .and_then(|b| b.get("mimeType"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                alt: m.get("alt").and_then(Value::as_str).map(str::to_owned),
                aspect_ratio: ar,
                duration_ms: m.get("durationMs").and_then(Value::as_u64),
                name: m.get("name").and_then(Value::as_str).map(str::to_owned),
                author: String::new(),
                pds: String::new(),
                // TODO(M4): the autoload policy (`sync.md` §11.3)
                autoload: false,
                size_bytes: blob
                    .and_then(|b| b.get("size"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            }
        })
        .collect()
}

/// Renders a markdown source with its `media[]` value (`defs#media` JSON) and `profile`.
pub fn render_markdown(src: &str, media: &Value, profile: Option<&str>) -> RenderedBody {
    let items: Vec<MediaItem> = media
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| serde_json::from_value(m.clone()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let opts = RenderOpts {
        html: true,
        styled: false,
        text: true,
    };
    let r = lessdugin_markdown::render_profile(src, &items, profile, &opts);
    from_rendered(r, media)
}

fn from_rendered(r: Rendered, media: &Value) -> RenderedBody {
    RenderedBody {
        notices: notices(&r),
        html: r.html,
        text: r.text,
        toc: r
            .toc
            .into_iter()
            .map(|t| TocEntry {
                level: t.level,
                id: t.id,
                text: t.text,
            })
            .collect(),
        word_count: r.word_count,
        read_minutes: r.read_minutes,
        media: media_views(media),
        excerpt: r.excerpt,
        renderer_version: renderer_version(),
    }
}

/// The CID of an `md` value's `textBlob` (`None`: an inline body).
pub fn text_blob_cid(md: &Value) -> Option<&str> {
    md.get("textBlob")?.get("ref")?.get("$link")?.as_str()
}

/// A spilled body as the engine fetched it (`lfm-1.md` §1.3), by its blob's CID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spilled {
    /// The decoded source (UTF-8, one leading U+FEFF stripped).
    Text(Arc<str>),
    /// It cannot be rendered: not UTF-8, or larger than the lexicon allows.
    Error(lessdugin_markdown::BodyError),
    /// The bytes did not match the CID (`data-model.md` §5.2 rule 3).
    Unverified,
}

/// Renders a spilled body from what was fetched for it.
fn render_spilled(md: &Value, spilled: &Spilled) -> RenderedBody {
    let media = md.get("media").cloned().unwrap_or(Value::Null);
    let profile = md.get("profile").and_then(Value::as_str);
    match spilled {
        Spilled::Text(src) => render_markdown(src, &media, profile),
        Spilled::Error(e) => from_rendered(lessdugin_markdown::render_error(*e, profile), &media),
        Spilled::Unverified => RenderedBody {
            notices: vec![NOTICE_UNVERIFIED_BODY.to_owned()],
            media: media_views(&media),
            ..empty_body()
        },
    }
}

/// Renders an `md` union value; `excerpt` is the record's own excerpt, the text of a spilled
/// body while it is pending (this function never fetches; [`RenderCache::body`] renders a
/// fetched one).
pub fn render_md(md: &Value, ns: &str, excerpt: Option<&str>) -> RenderedBody {
    let markdown_type = format!("{ns}.markdown");
    let known = md
        .get("$type")
        .and_then(Value::as_str)
        .is_some_and(|t| t == markdown_type || t == format!("{markdown_type}#main"));
    if !known {
        return RenderedBody {
            notices: vec![NOTICE_UNKNOWN_BODY.to_owned()],
            ..empty_body()
        };
    }
    let media = md.get("media").cloned().unwrap_or(Value::Null);
    let profile = md.get("profile").and_then(Value::as_str);
    match md.get("text").and_then(Value::as_str) {
        Some(text) => render_markdown(text, &media, profile),
        None => {
            // pending: the engine fetches the `textBlob` (`lfm-1.md` §1.3) before the views
            let text = excerpt.unwrap_or_default().to_owned();
            RenderedBody {
                excerpt: text.clone(),
                text,
                notices: vec![NOTICE_SPILLED.to_owned()],
                media: media_views(&media),
                ..empty_body()
            }
        }
    }
}

struct Lru {
    cap: usize,
    tick: u64,
    map: BTreeMap<(String, u32), (u64, Arc<RenderedBody>)>,
    order: BTreeMap<u64, (String, u32)>,
}

impl Lru {
    fn get(&mut self, k: &(String, u32)) -> Option<Arc<RenderedBody>> {
        self.tick += 1;
        let tick = self.tick;
        let (t, v) = self.map.get_mut(k)?;
        self.order.remove(t);
        *t = tick;
        self.order.insert(tick, k.clone());
        Some(Arc::clone(v))
    }

    fn put(&mut self, k: (String, u32), v: Arc<RenderedBody>) {
        self.tick += 1;
        if let Some((t, _)) = self.map.insert(k.clone(), (self.tick, v)) {
            self.order.remove(&t);
        }
        self.order.insert(self.tick, k);
        while self.map.len() > self.cap {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.map.remove(&oldest);
        }
    }
}

/// The spilled bodies fetched (by blob CID), at most [`SPILLED_ENTRIES`], oldest dropped first,
/// and the ones that could not be fetched, with the time to ask again.
#[derive(Default)]
struct SpilledMap {
    tick: u64,
    texts: BTreeMap<String, (u64, Arc<Spilled>)>,
    retry_at: BTreeMap<String, i64>,
}

/// Spilled sources kept in memory: each is at most `BLOB_TEXT_MAX_BYTES`, and once rendered the
/// body is in the LRU anyway (the host still has the verified bytes).
pub const SPILLED_ENTRIES: usize = 16;

/// The render cache: `(cid, RENDERER_VERSION) → RenderedBody`, least recently used dropped
/// first, and the fetched sources of spilled bodies.
pub struct RenderCache {
    lru: Mutex<Lru>,
    spilled: Mutex<SpilledMap>,
}

impl std::fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderCache")
            .field("len", &self.len())
            .finish()
    }
}

impl Default for RenderCache {
    fn default() -> Self {
        Self::new(crate::limits::RENDER_CACHE_ENTRIES)
    }
}

impl RenderCache {
    /// A cache of at most `cap` bodies.
    pub fn new(cap: usize) -> Self {
        Self {
            lru: Mutex::new(Lru {
                cap: cap.max(1),
                tick: 0,
                map: BTreeMap::new(),
                order: BTreeMap::new(),
            }),
            spilled: Mutex::new(SpilledMap::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Lru> {
        self.lru
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn spilled_map(&self) -> std::sync::MutexGuard<'_, SpilledMap> {
        self.spilled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `true` when the body of record `cid` is rendered and cached.
    pub fn contains(&self, cid: &str) -> bool {
        self.lock()
            .map
            .contains_key(&(cid.to_owned(), RENDERER_VERSION))
    }

    /// What was fetched for the spilled body whose blob is `blob` (`None`: pending).
    pub fn spilled(&self, blob: &str) -> Option<Arc<Spilled>> {
        self.spilled_map()
            .texts
            .get(blob)
            .map(|(_, v)| Arc::clone(v))
    }

    /// Records what was fetched for the blob `blob`.
    pub fn put_spilled(&self, blob: &str, v: Spilled) {
        let mut m = self.spilled_map();
        m.tick += 1;
        let tick = m.tick;
        m.retry_at.remove(blob);
        m.texts.insert(blob.to_owned(), (tick, Arc::new(v)));
        while m.texts.len() > SPILLED_ENTRIES {
            let Some(oldest) = m
                .texts
                .iter()
                .min_by_key(|(_, (t, _))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            m.texts.remove(&oldest);
        }
    }

    /// When the blob `blob`, which could not be fetched, may be asked for again (ms).
    pub fn spilled_retry_at(&self, blob: &str) -> Option<i64> {
        self.spilled_map().retry_at.get(blob).copied()
    }

    /// The blob `blob` could not be fetched: it stays pending until `at_ms`.
    pub fn spilled_unavailable(&self, blob: &str, at_ms: i64) {
        self.spilled_map().retry_at.insert(blob.to_owned(), at_ms);
    }

    /// Entries held.
    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    /// `true` if empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The cached body of record `cid`, rendering it with `render` on a miss (outside the
    /// lock).
    pub fn get_or_render(
        &self,
        cid: &str,
        render: impl FnOnce() -> RenderedBody,
    ) -> Arc<RenderedBody> {
        let key = (cid.to_owned(), RENDERER_VERSION);
        if let Some(v) = self.lock().get(&key) {
            return v;
        }
        let v = Arc::new(render());
        self.lock().put(key, Arc::clone(&v));
        v
    }

    /// The body of the record `cid` (whose body is `md`), with media views attributed to
    /// `author` at `pds` and their autoload verdict for `item` (`sync.md` §11.3). A spilled
    /// body renders from its fetched source; while that is pending, the uncached pending body.
    #[allow(clippy::too_many_arguments)]
    pub fn body(
        &self,
        cid: &str,
        md: &Value,
        ns: &str,
        excerpt: Option<&str>,
        author: &str,
        pds: &str,
        item: &BodyItem<'_>,
    ) -> RenderedBody {
        let spilled = text_blob_cid(md).filter(|_| md.get("text").is_none());
        let cached = match spilled.map(|b| self.spilled(b)) {
            None => self.get_or_render(cid, || render_md(md, ns, excerpt)),
            Some(Some(v)) => self.get_or_render(cid, || render_spilled(md, &v)),
            // pending: what an earlier fetch rendered, or the notice (never cached)
            Some(None) => {
                let earlier = self.lock().get(&(cid.to_owned(), RENDERER_VERSION));
                earlier.unwrap_or_else(|| Arc::new(render_md(md, ns, excerpt)))
            }
        };
        let mut b = (*cached).clone();
        let host = policy::host_of(pds);
        let shared = item.prefs.hosts.get(&host).copied().unwrap_or(0);
        for m in &mut b.media {
            author.clone_into(&mut m.author);
            pds.clone_into(&mut m.pds);
            let candidate = MediaCandidate {
                author,
                visible: item.visible,
                labels: item.labels,
                root_labels: item.root_labels,
                host: &host,
                mime: &m.mime,
                size_bytes: (m.size_bytes > 0).then_some(m.size_bytes),
            };
            let established = item.established && item.prefs.doc.contains(author);
            m.autoload = item.prefs.enabled
                && policy::autoload(&candidate, established, shared, &item.prefs.allowlist)
                    == Autoload::Yes;
        }
        b
    }
}

/// The device's media autoload inputs (`sync.md` §11.3): the master switch, the reader's
/// allowlist and how many of the forum's participants each PDS host serves. Per device, so it
/// is part of the viewer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaPrefs {
    /// The `mediaAutoload` setting is not `off`; off, nothing loads without a click.
    pub enabled: bool,
    /// `mediaHosts`: hosts the reader allowlisted.
    pub allowlist: Vec<String>,
    /// Participants of the forum by the host of the PDS in their identity rows.
    pub hosts: BTreeMap<String, usize>,
    /// The participants whose identity is at level `doc` (§11.1: a hint never autoloads).
    pub doc: std::collections::BTreeSet<String>,
}

/// What the autoload policy needs to know of the item a body belongs to (`sync.md` §11.3).
#[derive(Clone, Copy, Debug)]
pub struct BodyItem<'a> {
    /// The author is in S ∪ E of the forum.
    pub established: bool,
    /// The item is `Visible` after the lens (a `Collapsed` item's media stay click-to-load).
    pub visible: bool,
    /// The item's self-labels.
    pub labels: &'a [String],
    /// For a comment, its root post's self-labels.
    pub root_labels: &'a [String],
    /// The device's inputs.
    pub prefs: &'a MediaPrefs,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_inline_spilled_and_unknown() {
        let ns = lessdugin_lexicon::ns::NS;
        let md = json!({"$type": format!("{ns}.markdown"), "text": "# Hi\n\nSome *text*."});
        let b = render_md(&md, ns, None);
        assert!(b.html.starts_with("<div class=\"lfm\""));
        assert_eq!(b.text, "Hi Some text.");
        assert_eq!(b.toc.len(), 1);
        assert_eq!(b.renderer_version, renderer_version());
        let spilled = json!({"$type": format!("{ns}.markdown"), "textBlob": {}});
        let s = render_md(&spilled, ns, Some("An excerpt"));
        assert_eq!(s.text, "An excerpt");
        assert_eq!(s.notices, [NOTICE_SPILLED]);
        let u = render_md(&json!({"$type": "x.y"}), ns, None);
        assert_eq!(u.notices, [NOTICE_UNKNOWN_BODY]);
    }

    #[test]
    fn lru_keeps_the_recent_ones() {
        let c = RenderCache::new(2);
        let mut renders = 0;
        let mut get = |cid: &str| {
            c.get_or_render(cid, || {
                renders += 1;
                empty_body()
            });
        };
        get("a");
        get("b");
        get("a");
        get("c"); // evicts b
        get("a");
        get("b");
        assert_eq!(renders, 4);
    }
}
