// SPDX-License-Identifier: AGPL-3.0-or-later
//! Blobs of a write (`data-model.md` §8.8) and spilling of long bodies (§5.3, §8.9).
//!
//! - **MIME normalisation.** The `mimeType` declared to `uploadBlob` comes from the *sniffed*
//!   content through the §8.8 table ([`normalise_mime`]); anything else is refused.
//! - **Placeholders.** Until a blob is uploaded, the record holds a placeholder: the blob object
//!   computed locally (`{"$type":"blob","ref":{"$link":cid},"mimeType","size"}`). When the write is
//!   sent, the object `uploadBlob` returns replaces every placeholder with that CID, verbatim
//!   ([`substitute_blob`]); its `ref.$link` must equal the local CID.
//! - **Spilling.** A spillable `md` field (`post.body`, `wikiRevision.body`, `forum.about`,
//!   `sequence.body`) whose inline source exceeds 64 KiB is written as a `text/markdown`
//!   `textBlob` ([`spill_oversized`]); a request body over 900,000 bytes spills the largest
//!   remaining inline field until it fits ([`largest_inline`], [`spill`]). A spilled post body
//!   requires `excerpt` (PO6).

use lessdugin_lexicon::collection::nsid;
use serde_json::{Map, Value, json};

use crate::engine::EngineError;
use crate::limits::SPILL_THRESHOLD_BYTES;

/// The declared type of spilled LFM text.
pub const TEXT_MARKDOWN: &str = "text/markdown";

/// The declared `mimeType` for sniffed content (`data-model.md` §8.8), or `None` (refused).
pub fn normalise_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return Some(match &b[8..12] {
            b"M4A " | b"M4B " => "audio/mp4",
            _ => "video/mp4",
        });
    }
    if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("video/webm");
    }
    if b.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    if b.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if b.starts_with(b"ID3") || (b.len() >= 2 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0) {
        return Some("audio/mpeg");
    }
    None
}

/// A blob a write uploads when it is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobData {
    /// The local CID (CIDv1, raw, sha2-256).
    pub cid: String,
    /// The declared MIME type.
    pub mime: String,
    /// The bytes.
    pub bytes: Vec<u8>,
}

impl BlobData {
    /// Media bytes after the media pipeline: the type is sniffed and normalised (§8.8); a type
    /// outside the table is refused.
    pub fn media(bytes: Vec<u8>) -> Result<Self, EngineError> {
        let mime = normalise_mime(&bytes).ok_or_else(|| {
            EngineError::invalid_input("unsupported media type (data-model.md §8.8)")
                .with_detail("kind", "mediaRejected")
        })?;
        Ok(Self::with_mime(bytes, mime))
    }

    /// Spilled LFM source (`text/markdown`).
    pub fn text(text: &str) -> Self {
        Self::with_mime(text.as_bytes().to_vec(), TEXT_MARKDOWN)
    }

    fn with_mime(bytes: Vec<u8>, mime: &str) -> Self {
        Self {
            cid: lessdugin_atp::cid::for_raw(&bytes).to_string(),
            mime: mime.to_owned(),
            bytes,
        }
    }

    /// The locally computed blob object, which stands in the record until the upload.
    pub fn placeholder(&self) -> Value {
        placeholder(&self.cid, &self.mime, self.bytes.len() as u64)
    }
}

/// A blob object `{"$type": "blob", "ref": {"$link": cid}, "mimeType", "size"}`.
pub fn placeholder(cid: &str, mime: &str, size: u64) -> Value {
    json!({
        "$type": "blob",
        "ref": { "$link": cid },
        "mimeType": mime,
        "size": size,
    })
}

/// The `ref.$link` of a typed blob object.
pub fn blob_link(v: &Value) -> Option<&str> {
    let m = v.as_object()?;
    if m.get("$type").and_then(Value::as_str) != Some("blob") {
        return None;
    }
    m.get("ref")?.get("$link")?.as_str()
}

/// Replaces every typed blob object in `v` whose `ref.$link` is `cid` by `object`; returns how
/// many were replaced.
pub fn substitute_blob(v: &mut Value, cid: &str, object: &Value) -> usize {
    if blob_link(v) == Some(cid) {
        *v = object.clone();
        return 1;
    }
    match v {
        Value::Object(m) => m
            .values_mut()
            .map(|x| substitute_blob(x, cid, object))
            .sum(),
        Value::Array(a) => a.iter_mut().map(|x| substitute_blob(x, cid, object)).sum(),
        _ => 0,
    }
}

/// The spillable `md` fields of a collection (`data-model.md` §5.2 rule 2).
pub fn spillable_fields(collection: &str) -> &'static [&'static str] {
    if collection == nsid::POST || collection == nsid::WIKI_REVISION || collection == nsid::SEQUENCE
    {
        &["body"]
    } else if collection == nsid::FORUM {
        &["about"]
    } else {
        &[]
    }
}

/// The inline source of `value[field]` if it is an `NS.markdown` member with `text`.
pub fn inline_text<'v>(value: &'v Value, field: &str) -> Option<&'v str> {
    let md = value.get(field)?.as_object()?;
    if md.get("$type").and_then(Value::as_str) != Some(nsid::MARKDOWN) {
        return None;
    }
    md.get("text")?.as_str()
}

/// Spills `value[field]` (an inline `NS.markdown` member) into a `text/markdown` blob: `text` is
/// replaced by a `textBlob` placeholder. A spilled post body needs `excerpt` (PO6): `excerpt`
/// is written if the value has none, and without one the spill is refused. `Ok(None)` if the
/// field is not inline markdown.
pub fn spill(
    value: &mut Value,
    collection: &str,
    field: &str,
    excerpt: Option<&str>,
) -> Result<Option<BlobData>, EngineError> {
    let Some(text) = inline_text(value, field) else {
        return Ok(None);
    };
    let blob = BlobData::text(text);
    let is_post = collection == nsid::POST;
    let Some(obj) = value.as_object_mut() else {
        return Ok(None);
    };
    if is_post && !obj.contains_key("excerpt") {
        let e = excerpt.ok_or_else(|| {
            EngineError::invalid_input(
                "a post body longer than 64 KiB is spilled to a blob and needs an excerpt",
            )
        })?;
        obj.insert("excerpt".into(), Value::String(e.to_owned()));
    }
    if let Some(Value::Object(md)) = obj.get_mut(field) {
        md.remove("text");
        md.insert("textBlob".into(), blob.placeholder());
    }
    Ok(Some(blob))
}

/// Rule 1 (§5.3): spills every spillable field whose inline source exceeds 64 KiB.
pub fn spill_oversized(
    value: &mut Value,
    collection: &str,
    excerpt: Option<&str>,
) -> Result<Vec<BlobData>, EngineError> {
    let mut out = Vec::new();
    for field in spillable_fields(collection) {
        if inline_text(value, field).is_some_and(|t| t.len() > SPILL_THRESHOLD_BYTES)
            && let Some(b) = spill(value, collection, field, excerpt)?
        {
            out.push(b);
        }
    }
    Ok(out)
}

/// The largest inline spillable field among `values` (`(collection, value)` pairs): its index,
/// field name and source length.
pub fn largest_inline(values: &[(&str, &Value)]) -> Option<(usize, &'static str, usize)> {
    let mut best: Option<(usize, &'static str, usize)> = None;
    for (i, (coll, v)) in values.iter().enumerate() {
        for field in spillable_fields(coll) {
            if let Some(t) = inline_text(v, field)
                && best.is_none_or(|(_, _, n)| t.len() > n)
            {
                best = Some((i, field, t.len()));
            }
        }
    }
    best
}

/// Every typed blob `ref.$link` in `v` (any depth), deduplicated, in first-seen order.
pub fn blob_links(v: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        if let Some(l) = blob_link(v) {
            if !out.iter().any(|x| x == l) {
                out.push(l.to_owned());
            }
            return;
        }
        match v {
            Value::Object(m) => m.values().for_each(|x| walk(x, out)),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

/// A JSON object, or an error naming what was expected.
pub fn as_object<'v>(v: &'v Value, what: &str) -> Result<&'v Map<String, Value>, EngineError> {
    v.as_object()
        .ok_or_else(|| EngineError::invalid_input(format!("{what} is not a JSON object")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_table() {
        assert_eq!(
            normalise_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(normalise_mime(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(normalise_mime(b"GIF89a.."), Some("image/gif"));
        assert_eq!(normalise_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(normalise_mime(b"\0\0\0\x20ftypM4A \0\0"), Some("audio/mp4"));
        assert_eq!(normalise_mime(b"\0\0\0\x20ftypM4B \0\0"), Some("audio/mp4"));
        assert_eq!(normalise_mime(b"\0\0\0\x20ftypisom\0\0"), Some("video/mp4"));
        assert_eq!(
            normalise_mime(&[0x1A, 0x45, 0xDF, 0xA3, 1]),
            Some("video/webm")
        );
        assert_eq!(normalise_mime(b"ID3\x04"), Some("audio/mpeg"));
        assert_eq!(normalise_mime(&[0xFF, 0xFB, 0x90]), Some("audio/mpeg"));
        assert_eq!(normalise_mime(b"OggS\0"), Some("audio/ogg"));
        assert_eq!(normalise_mime(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(normalise_mime(b"<svg xmlns="), None);
        assert_eq!(normalise_mime(b"hello"), None);
        assert!(BlobData::media(b"<html>".to_vec()).is_err());
        let png = BlobData::media(b"\x89PNG\r\n\x1a\nabc".to_vec()).unwrap();
        assert_eq!(png.mime, "image/png");
        assert!(png.cid.starts_with("bafkrei"));
        assert_eq!(blob_link(&png.placeholder()), Some(png.cid.as_str()));
    }

    #[test]
    fn spill_rules() {
        let big = "x".repeat(SPILL_THRESHOLD_BYTES + 1);
        let small = "y".repeat(10);
        let mut post = json!({
            "$type": nsid::POST,
            "title": "t",
            "body": {"$type": nsid::MARKDOWN, "profile": "lfm/1", "text": big},
        });
        assert!(spill_oversized(&mut post.clone(), nsid::POST, None).is_err());
        let blobs = spill_oversized(&mut post, nsid::POST, Some("ex")).unwrap();
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].mime, TEXT_MARKDOWN);
        assert_eq!(post["excerpt"], "ex");
        assert!(post["body"].get("text").is_none());
        assert_eq!(
            blob_link(&post["body"]["textBlob"]),
            Some(blobs[0].cid.as_str())
        );
        assert_eq!(blob_links(&post), vec![blobs[0].cid.clone()]);
        // under the threshold nothing spills
        let mut seq =
            json!({"$type": nsid::SEQUENCE, "body": {"$type": nsid::MARKDOWN, "text": small}});
        assert!(
            spill_oversized(&mut seq, nsid::SEQUENCE, None)
                .unwrap()
                .is_empty()
        );
        // comments never spill
        let c = json!({"$type": nsid::COMMENT, "body": {"$type": nsid::MARKDOWN, "text": "z".repeat(70_000)}});
        assert!(largest_inline(&[(nsid::COMMENT, &c)]).is_none());
        assert_eq!(
            largest_inline(&[(nsid::COMMENT, &c), (nsid::SEQUENCE, &seq)]),
            Some((1, "body", 10))
        );
        // substitution is verbatim
        let ret = json!({"$type": "blob", "ref": {"$link": blobs[0].cid}, "mimeType": "text/plain", "size": 1, "x": 1});
        assert_eq!(substitute_blob(&mut post, &blobs[0].cid, &ret), 1);
        assert_eq!(post["body"]["textBlob"], ret);
    }
}
