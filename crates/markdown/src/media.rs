// SPDX-License-Identifier: AGPL-3.0-or-later
//! Body media (`docs/spec/lfm-1.md` §6): the `NS.markdown` `media[]` items that `cid:` URLs
//! resolve against, and the values the writers derive from them.

use serde::{Deserialize, Serialize};

use crate::event::{Ev, ImageRes, Tag};

/// `{"$link": cid}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CidLink {
    /// The CID string.
    #[serde(rename = "$link")]
    pub link: String,
}

/// The blob object of a `defs#media` (only the fields the renderer reads).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// `ref.$link`.
    #[serde(rename = "ref")]
    pub cid: CidLink,
    /// The declared MIME type.
    #[serde(rename = "mimeType", default)]
    pub mime_type: String,
}

/// `defs#aspectRatio`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AspectRatio {
    /// Width (1..=1,000,000 per the lexicon).
    pub width: u64,
    /// Height (1..=1,000,000 per the lexicon).
    pub height: u64,
}

/// One `defs#media` item, deserialisable from the record JSON (unknown fields are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItem {
    /// The blob reference.
    pub blob: BlobRef,
    /// Accessible description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    /// Layout hint.
    #[serde(
        rename = "aspectRatio",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub aspect_ratio: Option<AspectRatio>,
    /// Opt-in display-only basename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// How a resolved media item renders (§6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaClass {
    /// An inline image type: `<img>`.
    Image,
    /// `video/*`.
    Video,
    /// `audio/*`.
    Audio,
    /// Anything else.
    File,
}

impl MediaItem {
    /// `blob.ref.$link`.
    pub fn cid(&self) -> &str {
        &self.blob.cid.link
    }

    /// The §6.2 class of the declared MIME type, compared exactly as written (no case folding,
    /// no parameters), like the table in §6.2.
    pub(crate) fn class(&self) -> MediaClass {
        let mime = self.blob.mime_type.as_str();
        if matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif"
        ) {
            MediaClass::Image
        } else if mime.starts_with("video/") {
            MediaClass::Video
        } else if mime.starts_with("audio/") {
            MediaClass::Audio
        } else {
            MediaClass::File
        }
    }

    /// `A` (§6.2) of one reference: the markdown alt if non-empty after trimming, else
    /// `media.alt` if this reference may fall back to it (the first rendered reference that
    /// needs it, see [`assign_alt_fallback`]), else `""`. Always trimmed with [`trim_alt`].
    pub(crate) fn alt_text(&self, markdown_alt: &str, fallback: bool) -> String {
        let md = trim_alt(markdown_alt);
        if !md.is_empty() {
            md.to_owned()
        } else if fallback {
            self.gallery_alt()
        } else {
            String::new()
        }
    }

    /// `A` of a gallery item (§6.3): `media.alt`, trimmed.
    pub(crate) fn gallery_alt(&self) -> String {
        trim_alt(self.alt.as_deref().unwrap_or("")).to_owned()
    }

    /// `L` (§6.2): `A` if non-empty, else an acceptable `name`, else `""`.
    pub(crate) fn label(&self, a: &str) -> String {
        if !a.is_empty() {
            return a.to_owned();
        }
        match &self.name {
            Some(n) if name_acceptable(n) => n.clone(),
            _ => String::new(),
        }
    }

    /// `(W, H)` (§6.2): the aspect ratio clamped to between 1:20 and 20:1, if present and
    /// representable under the §8.2 grammar.
    pub(crate) fn dims(&self) -> Option<(u64, u64)> {
        let ar = self.aspect_ratio?;
        let (mut w, mut h) = (ar.width, ar.height);
        if w == 0 || h == 0 {
            return None;
        }
        if w > h.saturating_mul(20) {
            w = h * 20;
        }
        if h > w.saturating_mul(20) {
            h = w * 20;
        }
        (w <= 99_999_999 && h <= 99_999_999).then_some((w, h))
    }
}

/// Trim the white space that DOMPurify strips from attribute values (ECMAScript
/// `String.prototype.trim`: White_Space plus U+FEFF), so that `alt` survives sanitising
/// unchanged (§8.3).
pub(crate) fn trim_alt(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

/// Mark, in render order (the body, then the rendered footnotes), the first reference to each
/// media item whose markdown alt is empty: only that reference shows `media.alt` (§6.2). A
/// body that references one item many times can then not repeat its (up to 20,000-byte)
/// `alt` record field in every reference.
pub(crate) fn assign_alt_fallback<'a>(flows: impl IntoIterator<Item = &'a mut [Ev]>, n: usize) {
    let mut taken = vec![false; n];
    for flow in flows {
        for e in flow.iter_mut() {
            if let Ev::Start(Tag::Image(d)) = e
                && let ImageRes::Media(i) = d.res
                && trim_alt(&d.alt).is_empty()
                && let Some(t) = taken.get_mut(i)
            {
                d.fallback_alt = !*t;
                *t = true;
            }
        }
    }
}

/// `data-model.md` §4: no `/`, `\`, C0, C1, U+2028 or U+2029.
pub(crate) fn name_acceptable(n: &str) -> bool {
    !n.chars().any(|c| {
        matches!(c, '/' | '\\' | '\u{0}'..='\u{1F}' | '\u{80}'..='\u{9F}' | '\u{2028}' | '\u{2029}')
    })
}

/// `media_used` (§6.3): every index whose CID equals a resolved `cid:` URL, ascending.
pub(crate) fn media_used(media: &[MediaItem], resolved: &[usize]) -> Vec<u16> {
    let cids: std::collections::BTreeSet<&str> = resolved.iter().map(|&i| media[i].cid()).collect();
    media
        .iter()
        .enumerate()
        .filter(|(_, m)| cids.contains(m.cid()))
        .filter_map(|(i, _)| u16::try_from(i).ok())
        .collect()
}
