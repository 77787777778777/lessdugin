// SPDX-License-Identifier: AGPL-3.0-or-later
//! The LFM event vector (`docs/spec/lfm-1.md` §1.5): an owned, flat, balanced sequence of
//! start/end tags and leaves. Every stage is a forward iteration over such a vector with
//! explicit stacks; nothing recurses in proportion to input nesting.

use serde::Serialize;

use crate::at::{AtUri, Did};

/// GFM alert kinds (`> [!NOTE]` …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertKind {
    /// `[!NOTE]`
    Note,
    /// `[!TIP]`
    Tip,
    /// `[!IMPORTANT]`
    Important,
    /// `[!WARNING]`
    Warning,
    /// `[!CAUTION]`
    Caution,
}

impl AlertKind {
    pub(crate) fn class(self) -> &'static str {
        match self {
            Self::Note => "alert-note",
            Self::Tip => "alert-tip",
            Self::Important => "alert-important",
            Self::Warning => "alert-warning",
            Self::Caution => "alert-caution",
        }
    }
}

/// Table column alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// No alignment given.
    None,
    /// `:--`
    Left,
    /// `:-:`
    Center,
    /// `--:`
    Right,
}

/// The source form of a link or image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkKind {
    Inline,
    Reference,
    Collapsed,
    Shortcut,
    /// `<scheme:…>`
    Autolink,
    /// `<a@b.c>`; the destination gets `mailto:` prefixed (§4.5).
    Email,
    /// GFM extended autolink created by pass 1 (§4.1).
    ExtAutolink,
}

/// Pass-5 resolution of a link (§4.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkRes {
    /// Before pass 5.
    Pending,
    External {
        url: String,
        host: String,
    },
    Mailto {
        url: String,
        host: String,
    },
    Record(AtUri),
    Profile(Did),
    Mention(Did),
    Anchor(String),
    /// Rendered as its inner content only.
    Dropped,
}

/// Pass-5 resolution of an image (§4.5, §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImageRes {
    Pending,
    /// `cid:` hit: lowest matching index into `media`.
    Media(usize),
    /// `cid:` miss (no match, or a malformed CID).
    Missing,
    /// `http(s)` accepted by `safe_href`; never fetched.
    External {
        url: String,
        host: String,
    },
    /// Any other destination: the alt text as plain text.
    AltOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkData {
    pub kind: LinkKind,
    pub dest: String,
    pub title: String,
    /// `plain(inner events)` (§10.1, no normalisation); filled by pass 5.
    pub inner: String,
    pub res: LinkRes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageData {
    pub kind: LinkKind,
    pub dest: String,
    pub title: String,
    /// `plain(inner events)`; filled by pass 5.
    pub alt: String,
    pub res: ImageRes,
    /// Whether an empty markdown alt may fall back to `media.alt` (§6.2): only for the first
    /// rendered reference to a media item that needs it ([`crate::media::assign_alt_fallback`]).
    pub fallback_alt: bool,
}

/// Container tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    Paragraph,
    /// `id` is assigned by pass 6 (without the `md-h-` prefix it is the slug).
    Heading {
        level: u8,
        id: String,
    },
    BlockQuote(Option<AlertKind>),
    /// `None` for indented code, else the (CR-normalised) info string.
    CodeBlock(Option<String>),
    List(Option<u64>),
    Item,
    /// `ord` is the rendered ordinal (pass 6), or `None` if the definition is not rendered.
    FootnoteDef {
        label: String,
        ord: Option<u32>,
    },
    Table(Vec<Align>),
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Link(Box<LinkData>),
    Image(Box<ImageData>),
    Spoiler,
    Details,
    Summary,
    Figure(String),
}

/// End-tag kinds (data-free mirror of [`Tag`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagKind {
    Paragraph,
    Heading,
    BlockQuote,
    CodeBlock,
    List,
    Item,
    FootnoteDef,
    Table,
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Link,
    Image,
    Spoiler,
    Details,
    Summary,
    Figure,
}

impl Tag {
    pub(crate) fn kind(&self) -> TagKind {
        match self {
            Tag::Paragraph => TagKind::Paragraph,
            Tag::Heading { .. } => TagKind::Heading,
            Tag::BlockQuote(_) => TagKind::BlockQuote,
            Tag::CodeBlock(_) => TagKind::CodeBlock,
            Tag::List(_) => TagKind::List,
            Tag::Item => TagKind::Item,
            Tag::FootnoteDef { .. } => TagKind::FootnoteDef,
            Tag::Table(_) => TagKind::Table,
            Tag::TableHead => TagKind::TableHead,
            Tag::TableRow => TagKind::TableRow,
            Tag::TableCell => TagKind::TableCell,
            Tag::Emphasis => TagKind::Emphasis,
            Tag::Strong => TagKind::Strong,
            Tag::Strikethrough => TagKind::Strikethrough,
            Tag::Link(_) => TagKind::Link,
            Tag::Image(_) => TagKind::Image,
            Tag::Spoiler => TagKind::Spoiler,
            Tag::Details => TagKind::Details,
            Tag::Summary => TagKind::Summary,
            Tag::Figure(_) => TagKind::Figure,
        }
    }
}

impl TagKind {
    /// Block-level tags (a flattening boundary, §8.4; `"\n"` in P, §10.1).
    pub(crate) fn is_block(self) -> bool {
        !matches!(
            self,
            TagKind::Emphasis
                | TagKind::Strong
                | TagKind::Strikethrough
                | TagKind::Link
                | TagKind::Image
        )
    }
}

/// A footnote reference. `ord`/`k` are assigned by pass 6 (`0` = not rendered).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FnRef {
    pub label: String,
    pub ord: u32,
    pub k: u32,
}

/// One event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ev {
    Start(Tag),
    End(TagKind),
    Text(String),
    /// Raw HTML shown as text; never examined by post-passes (§3 step 2).
    Literal(String),
    Code(String),
    InlineMath(String),
    DisplayMath(String),
    FnRef(FnRef),
    SoftBreak,
    HardBreak,
    Rule,
    Task(bool),
}

/// For every `Start` at index i, `m[i]` is the index of its matching `End` (and vice versa);
/// leaves map to themselves. Unbalanced input (never produced) maps to the vector length.
pub(crate) fn match_indices(evs: &[Ev]) -> Vec<usize> {
    let mut m = vec![evs.len(); evs.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, e) in evs.iter().enumerate() {
        match e {
            Ev::Start(_) => stack.push(i),
            Ev::End(_) => {
                if let Some(s) = stack.pop() {
                    m[s] = i;
                    m[i] = s;
                }
            }
            _ => m[i] = i,
        }
    }
    m
}

/// Push text, merging with a preceding `Text` (coalescing, §3 step 4).
pub(crate) fn push_text(out: &mut Vec<Ev>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Ev::Text(t)) = out.last_mut() {
        t.push_str(s);
    } else {
        out.push(Ev::Text(s.to_owned()));
    }
}

/// Push a literal, merging with a preceding `Literal`.
pub(crate) fn push_literal(out: &mut Vec<Ev>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Ev::Literal(t)) = out.last_mut() {
        t.push_str(s);
    } else {
        out.push(Ev::Literal(s.to_owned()));
    }
}
