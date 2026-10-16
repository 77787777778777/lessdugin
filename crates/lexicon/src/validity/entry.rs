// SPDX-License-Identifier: MIT OR Apache-2.0
//! Entry-level rules (`data-model.md` §7.3, rules marked *entry*): they discard one field, axis
//! or array entry and never invalidate the record. Each helper reads a (valid) typed record and
//! returns what survives. Rules that need other records (the react palette of OP3, CO6, the
//! "own thread" part of MA6) stay in the core fold.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::{AtUri, Did};

use crate::collection::Collection;
use crate::keys;
use crate::records::{
    Chapter, Comment, Endpoint, ExternalLink, Forum, LockAction, Media, Moderator, Opinion, Post,
    React, RestrictAction, Sequence, StrongRef, WikiRevision,
};
use crate::validate::accept_matches;
use crate::values::{Capability, PostKind, Role, Stance, VoteValue};

/// FO2: the effective moderators of a forum owned by `owner`, in order. The first entry for a
/// DID wins (a later entry for the same DID is ignored even if the first one is ignored too);
/// entries for the owner and entries with an unknown role are ignored.
pub fn moderators<'a>(forum: &'a Forum, owner: &Did) -> Vec<(&'a Moderator, Role)> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for m in forum.moderators.iter().flatten() {
        if !seen.insert(m.did.as_str()) {
            continue;
        }
        if &m.did == owner {
            continue;
        }
        if let Some(role) = m.role() {
            out.push((m, role));
        }
    }
    out
}

/// FO3: `true` if an indexer endpoint URL may be used: `https:` (any case), a DNS hostname with
/// at least two labels (not an IP literal, not `localhost`), no userinfo and no explicit port.
pub fn usable_indexer_url(url: &str) -> bool {
    let Some(scheme_end) = url.find("://") else {
        return false;
    };
    if !url[..scheme_end].eq_ignore_ascii_case("https") {
        return false;
    }
    let rest = &url[scheme_end + 3..];
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || host.contains(['@', ':', '[', ']']) || host.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    labels.len() >= 2
        && labels.iter().all(label_ok)
        && !labels
            .last()
            .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
}

/// FO3: the usable `discovery.indexers` entries.
pub fn usable_indexers(forum: &Forum) -> Vec<&Endpoint> {
    forum
        .discovery
        .iter()
        .flat_map(|d| d.indexers.iter().flatten())
        .filter(|e| usable_indexer_url(&e.url))
        .collect()
}

/// Comment placement (structural part of `social.md` threads, not a validity rule): `true` if
/// the comment's `parent` is absent, or is an `NS.comment` uri in the same thread: its rkey is
/// `{fk}-{T}-…` with the comment's own fk and root TID. A comment whose parent fails this goes
/// under a `Missing(parent)` placeholder in the fold.
pub fn comment_parent_in_thread(comment: &Comment, comment_rkey: &str) -> bool {
    let Some(parent) = &comment.parent else {
        return true;
    };
    let Some((fk, root, _)) = keys::split_comment_key(comment_rkey) else {
        return false;
    };
    Collection::from_nsid(parent.uri.collection().as_str()) == Some(Collection::Comment)
        && keys::split_comment_key(parent.uri.rkey().as_str())
            .is_some_and(|(pfk, proot, _)| pfk == fk && proot == root)
}

/// PO4: the title to show and index (`None` for a quickTake, whose title is ignored).
pub fn post_title(post: &Post) -> Option<&str> {
    match post.kind().value {
        PostKind::QuickTake => None,
        _ => post.title.as_deref(),
    }
}

/// PO5: the link card (only for kind `link`).
pub fn post_link(post: &Post) -> Option<&ExternalLink> {
    match post.kind().value {
        PostKind::Link => post.link.as_ref(),
        _ => None,
    }
}

/// PO6: the excerpt, read only when the body is spilled.
pub fn post_excerpt(post: &Post) -> Option<&str> {
    let spilled = post
        .body
        .as_ref()
        .and_then(|b| b.known())
        .is_some_and(|m| m.is_spilled());
    if spilled {
        post.excerpt.as_deref()
    } else {
        None
    }
}

/// PO7: the author's tags as a set (duplicates collapse; first-occurrence order).
pub fn post_tags(post: &Post) -> Vec<&str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    post.tags
        .iter()
        .flatten()
        .map(String::as_str)
        .filter(|t| seen.insert(t))
        .collect()
}

/// PO8: the self-label values of the `com.atproto.label.defs#selfLabels` member (other members
/// are ignored), deduplicated. Which values are *known* is the reader's decision (§6.3 names
/// `sexual`, `nudity`, `porn`, `graphic-media`; see [`KNOWN_SELF_LABELS`]).
pub fn self_labels(post: &Post) -> Vec<&str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out: Vec<&str> = Vec::new();
    if let Some(sl) = post.labels.as_ref().and_then(|l| l.known()) {
        for v in &sl.values {
            if seen.insert(&v.val) {
                out.push(&v.val);
            }
        }
    }
    out
}

/// The self-label values `data-model.md` §6.3 names.
pub const KNOWN_SELF_LABELS: &[&str] = &["sexual", "nudity", "porn", "graphic-media"];

/// OP3: the react entries with a known stance (the palette check is the fold's), with their
/// stance.
pub fn opinion_reacts(op: &Opinion) -> Vec<(&React, Stance)> {
    op.reacts
        .iter()
        .flatten()
        .filter_map(|r| r.stance().map(|s| (r, s)))
        .collect()
}

/// OP3 + OP4: the tag votes that count. Empty unless the subject is a post. Entries with an
/// unknown value are discarded first (OP3); then several entries for one tag collapse if their
/// values are equal, and are all ignored otherwise (OP4). First-occurrence order.
pub fn opinion_tag_votes(op: &Opinion) -> Vec<(&str, VoteValue)> {
    let subject_is_post = keys::split_uri(op.subject.uri.as_str())
        .ok()
        .and_then(|(_, c, _)| Collection::from_nsid(c))
        == Some(Collection::Post);
    if !subject_is_post {
        return Vec::new();
    }
    let known: Vec<(&str, VoteValue)> = op
        .tags
        .iter()
        .flatten()
        .filter_map(|t| t.value().map(|v| (t.tag.as_str(), v)))
        .collect();
    // per tag: its first value, and whether every entry agrees with it
    let mut agree: BTreeMap<&str, (VoteValue, bool)> = BTreeMap::new();
    for &(tag, v) in &known {
        agree
            .entry(tag)
            .and_modify(|(first, same)| *same &= *first == v)
            .or_insert((v, true));
    }
    let mut out: Vec<(&str, VoteValue)> = Vec::new();
    for &(tag, _) in &known {
        if let Some((v, true)) = agree.remove(tag) {
            out.push((tag, v));
        }
    }
    out
}

/// MA8: the known restrict capabilities, as a set (unknown items ignored, duplicates collapse).
pub fn restrict_capabilities(r: &RestrictAction) -> BTreeSet<Capability> {
    r.capabilities
        .iter()
        .flatten()
        .filter_map(|c| Capability::parse(c))
        .collect()
}

/// MA9: the `allowed` entries inside the locked post's thread: `NS.comment` uris whose rkey is
/// `{fk}-{T}-…` with the post's fk and `T = threadTid(subject)`. Empty if `subject` is not an
/// `NS.post` uri with a canonical key.
pub fn lock_allowed<'a>(lock: &'a LockAction, subject: &str) -> Vec<&'a AtUri> {
    let Some((pfk, t)) = keys::split_uri(subject)
        .ok()
        .filter(|(_, c, _)| Collection::from_nsid(c) == Some(Collection::Post))
        .and_then(|(_, _, k)| keys::split_fk_tid(k))
    else {
        return Vec::new();
    };
    lock.allowed
        .iter()
        .flatten()
        .filter(|u| {
            Collection::from_nsid(u.collection().as_str()) == Some(Collection::Comment)
                && keys::split_comment_key(u.rkey().as_str())
                    .is_some_and(|(fk, root, _)| fk == pfk && root == t)
        })
        .collect()
}

/// WR2: the parents with duplicates (by uri) collapsed, first occurrence kept.
pub fn wiki_parents(w: &WikiRevision) -> Vec<&StrongRef> {
    let mut out: Vec<&StrongRef> = Vec::new();
    for p in w.parents.iter().flatten() {
        if !out.iter().any(|q| q.uri == p.uri) {
            out.push(p);
        }
    }
    out
}

/// SQ2: the chapter entries that are displayed: those whose rkey's fk is the sequence's
/// `fk(forum)`.
pub fn chapter_posts<'a>(seq: &Sequence, chapter: &'a Chapter) -> Vec<&'a AtUri> {
    let Ok(f) = keys::fk(seq.forum.as_str()) else {
        return Vec::new();
    };
    chapter
        .posts
        .iter()
        .filter(|u| keys::fk_of(u.rkey().as_str()) == Some(f.as_str()))
        .collect()
}

/// SQ3: the cover, displayed only if its blob's `mimeType` is `image/*`.
pub fn sequence_cover(seq: &Sequence) -> Option<&Media> {
    seq.cover
        .as_ref()
        .filter(|m| accept_matches("image/*", &m.blob.mime_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexer_urls() {
        for ok in [
            "https://index.example.com",
            "https://index.example.com/",
            "HTTPS://index.example.com/xrpc",
            "https://a.b.c.example/path?q=1",
        ] {
            assert!(usable_indexer_url(ok), "{ok}");
        }
        for bad in [
            "http://index.example.com",
            "https://index.example.com:443/",
            "https://user@index.example.com",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://localhost/",
            "https://",
            "index.example.com",
            "https://-a.example.com",
        ] {
            assert!(!usable_indexer_url(bad), "{bad}");
        }
    }
}
