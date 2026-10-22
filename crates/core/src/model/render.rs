// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rendering bodies through `lessdugin_markdown` into [`RenderedBody`] DTOs, with the in-memory
//! LRU keyed by `(cid, RENDERER_VERSION)` (`sync.md` §13, `lfm-1.md` §1.2).
//!
//! A body is an `NS.markdown` union value (`data-model.md` §5): inline `text`, or a `textBlob`.
//! Spilled bodies are fetched with the media work of M4; until then they render as a notice
//! with the record's `excerpt` as text. An unknown union member renders as a notice.
//!
//! The cached value does not depend on who renders it: the author and PDS of media views are
//! filled in per call ([`RenderCache::body`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lessdugin_markdown::{MediaItem, RENDERER_VERSION, RenderOpts, Rendered};
use serde_json::Value;

use crate::dto::{AspectRatio, MediaView, RenderedBody, TocEntry};

/// The presentation version as written in DTOs.
pub fn renderer_version() -> String {
    RENDERER_VERSION.to_string()
}

/// Notice for a spilled body that is not fetched yet (M4 fetches `textBlob`s).
pub const NOTICE_SPILLED: &str = "[the body is stored as a blob and is not loaded yet]";
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

/// Renders an `md` union value; `excerpt` is the record's own excerpt (shown for spilled
/// bodies until M4 fetches them).
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
            // TODO(M4): fetch the `textBlob` (`lfm-1.md` §1.3) and render it.
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

/// The render cache: `(cid, RENDERER_VERSION) → RenderedBody`, least recently used dropped
/// first.
pub struct RenderCache {
    lru: Mutex<Lru>,
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
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Lru> {
        self.lru
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    /// `author` at `pds`.
    pub fn body(
        &self,
        cid: &str,
        md: &Value,
        ns: &str,
        excerpt: Option<&str>,
        author: &str,
        pds: &str,
    ) -> RenderedBody {
        let cached = self.get_or_render(cid, || render_md(md, ns, excerpt));
        let mut b = (*cached).clone();
        for m in &mut b.media {
            author.clone_into(&mut m.author);
            pds.clone_into(&mut m.pds);
        }
        b
    }
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
