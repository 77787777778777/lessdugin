// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared definitions (`NS.defs`, §4), `NS.markdown` (§5), blobs, strong references and
//! self-labels.

use lessdugin_atp::syntax::{AtUri, Did};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{OpenUnion, UnionMember, int};
use crate::collection::nsid;

fn markdown_type() -> String {
    nsid::MARKDOWN.to_owned()
}

fn self_labels_type() -> String {
    nsid::SELF_LABELS.to_owned()
}

fn blob_type() -> String {
    nsid::BLOB.to_owned()
}

/// A `cid-link`: `{"$link": "<cid>"}` (exactly one key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CidLink {
    /// The CID string.
    #[serde(rename = "$link")]
    pub link: String,
}

/// A typed blob reference `{"$type": "blob", "ref", "mimeType", "size"}`, embedded verbatim
/// as returned by `uploadBlob` (§8.8). The legacy `{cid, mimeType}` form is invalid (G3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    /// Always `"blob"`.
    #[serde(rename = "$type", default = "blob_type")]
    pub r#type: String,
    /// The blob CID (raw codec).
    #[serde(rename = "ref")]
    pub r#ref: CidLink,
    /// The declared MIME type.
    pub mime_type: String,
    /// The size in bytes.
    #[serde(deserialize_with = "int::de")]
    pub size: i64,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Blob {
    /// A blob reference.
    pub fn new(cid: impl Into<String>, mime_type: impl Into<String>, size: i64) -> Self {
        Self {
            r#type: blob_type(),
            r#ref: CidLink { link: cid.into() },
            mime_type: mime_type.into(),
            size,
            extra: Map::new(),
        }
    }

    /// The blob CID, if it parses.
    pub fn cid(&self) -> Option<lessdugin_atp::Cid> {
        lessdugin_atp::cid::parse(&self.r#ref.link).ok()
    }
}

/// `com.atproto.repo.strongRef`: `{uri, cid}` (the `sref` of §2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrongRef {
    /// The canonical at-uri (§2.3).
    pub uri: AtUri,
    /// The CID of the referenced version.
    pub cid: String,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl StrongRef {
    /// A strong reference.
    pub fn new(uri: AtUri, cid: impl Into<String>) -> Self {
        Self {
            uri,
            cid: cid.into(),
            extra: Map::new(),
        }
    }
}

/// `defs#media` (§4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    /// The blob (any type, no size limit).
    pub blob: Blob,
    /// Accessible description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    /// Layout hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
    /// Duration (audio, video).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub duration_ms: Option<i64>,
    /// Display-only basename; see [`Media::display_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Media {
    /// A media item for a blob.
    pub fn new(blob: Blob) -> Self {
        Self {
            blob,
            alt: None,
            aspect_ratio: None,
            duration_ms: None,
            name: None,
            extra: Map::new(),
        }
    }

    /// `name`, unless readers must ignore it (§4): it contains `/`, `\`, a C0 or C1 control,
    /// U+2028 or U+2029.
    pub fn display_name(&self) -> Option<&str> {
        self.name.as_deref().filter(|n| {
            !n.chars().any(|c| {
                c == '/'
                    || c == '\\'
                    || c <= '\u{1f}'
                    || ('\u{80}'..='\u{9f}').contains(&c)
                    || c == '\u{2028}'
                    || c == '\u{2029}'
            })
        })
    }
}

/// `defs#aspectRatio` (§4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AspectRatio {
    /// Relative width, 1..=1,000,000.
    #[serde(deserialize_with = "int::de")]
    pub width: i64,
    /// Relative height, 1..=1,000,000.
    #[serde(deserialize_with = "int::de")]
    pub height: i64,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `defs#textQuote` (§4): a selection in the plain-text projection P of the target's body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextQuote {
    /// The quoted text, trimmed.
    pub exact: String,
    /// Verbatim text of P immediately before `exact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Verbatim text of P immediately after `exact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    /// Code-point offset hint.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub start: Option<i64>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TextQuote {
    /// A quote of `exact` only.
    pub fn new(exact: impl Into<String>) -> Self {
        Self {
            exact: exact.into(),
            prefix: None,
            suffix: None,
            start: None,
            extra: Map::new(),
        }
    }
}

/// `defs#externalLink` (§4): a link card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalLink {
    /// Target URL (`uri` format).
    pub uri: String,
    /// Card title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Card description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Card thumbnail (`image/*`, ≤ 1,000,000 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<Blob>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `defs#link` (§4): a labelled URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Target URL (`uri` format).
    pub uri: String,
    /// Link text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `NS.markdown` (§5): an LFM/1 body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Markdown {
    /// `NS.markdown` of this build.
    #[serde(rename = "$type", default = "markdown_type")]
    pub r#type: String,
    /// Dialect (`kv{"lfm/1"}`, default `lfm/1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Inline source (≤ 100,000 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Spilled source (`text/*`, ≤ 1,000,000 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_blob: Option<Blob>,
    /// The only scope for `cid:` URLs in this body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Vec<Media>>,
    /// Mentioned DIDs (writer-derived hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<Did>>,
    /// Linked at-uris (writer-derived hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<AtUri>>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Markdown {
    /// An inline `lfm/1` body (writers set `profile: "lfm/1"`, `lfm-1.md` §12).
    pub fn inline(text: impl Into<String>) -> Self {
        Self {
            r#type: markdown_type(),
            profile: Some(crate::values::MarkdownProfile::Lfm1.as_str().to_owned()),
            text: Some(text.into()),
            text_blob: None,
            media: None,
            mentions: None,
            links: None,
            extra: Map::new(),
        }
    }

    /// A spilled `lfm/1` body.
    pub fn spilled(blob: Blob) -> Self {
        Self {
            text: None,
            text_blob: Some(blob),
            ..Self::inline("")
        }
    }

    /// `true` if the body uses `textBlob` (spilled).
    pub fn is_spilled(&self) -> bool {
        self.text_blob.is_some()
    }
}

impl UnionMember for Markdown {
    const TYPE: &'static str = nsid::MARKDOWN;
}

/// An `md` value: the open union of `NS.markdown` (§2.1).
pub type Md = OpenUnion<Markdown>;

/// `com.atproto.label.defs#selfLabel`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfLabel {
    /// The label value (≤ 128 bytes).
    pub val: String,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `com.atproto.label.defs#selfLabels`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfLabels {
    /// `com.atproto.label.defs#selfLabels`.
    #[serde(rename = "$type", default = "self_labels_type")]
    pub r#type: String,
    /// The labels (≤ 10).
    pub values: Vec<SelfLabel>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SelfLabels {
    /// Self-labels with these values.
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(values: I) -> Self {
        Self {
            r#type: self_labels_type(),
            values: values
                .into_iter()
                .map(|v| SelfLabel {
                    val: v.into(),
                    extra: Map::new(),
                })
                .collect(),
            extra: Map::new(),
        }
    }
}

impl UnionMember for SelfLabels {
    const TYPE: &'static str = nsid::SELF_LABELS;
}

/// `post.labels`: the open union of `com.atproto.label.defs#selfLabels`.
pub type Labels = OpenUnion<SelfLabels>;
