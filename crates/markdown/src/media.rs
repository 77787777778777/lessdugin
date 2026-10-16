// SPDX-License-Identifier: AGPL-3.0-or-later
//! Body media (`docs/spec/lfm-1.md` §6): the `NS.markdown` `media[]` items that `cid:` URLs
//! resolve against, and the values the writers derive from them.

use serde::{Deserialize, Serialize};

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

    pub(crate) fn class(&self) -> MediaClass {
        let mime = self.blob.mime_type.to_ascii_lowercase();
        let mime = mime.split(';').next().unwrap_or("").trim();
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

    /// `A` (§6.2): the markdown alt if non-empty after trimming, else `media.alt`, else `""`.
    pub(crate) fn alt_text(&self, markdown_alt: &str) -> String {
        if !markdown_alt.trim().is_empty() {
            markdown_alt.to_owned()
        } else {
            self.alt.clone().unwrap_or_default()
        }
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

/// `data-model.md` §4: no `/`, `\`, C0, C1, U+2028 or U+2029.
pub(crate) fn name_acceptable(n: &str) -> bool {
    !n.chars().any(|c| {
        matches!(c, '/' | '\\' | '\u{0}'..='\u{1F}' | '\u{80}'..='\u{9F}' | '\u{2028}' | '\u{2029}')
    })
}

/// `media_used` (§6.3): every index whose CID equals a resolved `cid:` URL, ascending.
pub(crate) fn media_used(media: &[MediaItem], resolved: &[usize]) -> Vec<u16> {
    let cids: std::collections::HashSet<&str> = resolved.iter().map(|&i| media[i].cid()).collect();
    media
        .iter()
        .enumerate()
        .filter(|(_, m)| cids.contains(m.cid()))
        .filter_map(|(i, _)| u16::try_from(i).ok())
        .collect()
}
