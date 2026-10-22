// SPDX-License-Identifier: AGPL-3.0-or-later
//! Mod log, mod queue and raw-view annotations (`mod/1`, `social.md` §4.12).
//!
//! ```text
//! mod log  = every listed modAction (valid, forum = F; any scope, any actor) with its status
//!            ∪ every valid revision of an NS.modAction (edited: op update; removed: op delete),
//!              whether or not the modAction is still in records
//!            order (time desc, uri asc); entries without a time last, by uri
//!            time = µs(updatedAt) ?? µs(createdAt) (modAction) | µs(createdAt) (revision)
//! status   = reserved/unknown (no effect, either scope) | forum scope: §4.3 order (L1)
//!            | author scope: §4.9 (L5)
//! queue    = applicants (approval mode) ∪ top-rank tag.head conflicts
//!            ∪ stale forum-scope records whose subject (or head.uri) is in records with another
//!              CID; sorted by (type, key bytes)
//! raw      = the first failing rule of every invalid record; every listed modAction by subject
//! ```

use std::collections::BTreeMap;

use lessdugin_lexicon::Collection;

use crate::social::full::author::AuthorTier;
use crate::social::full::cx::Cx;
use crate::social::full::moderation::{ModScope, Moderation};
use crate::social::full::view::{
    ModLogEntry, ModStatus, Participation, QueueItem, Raw, Stage,
};
use crate::social::input::SkelData;
use crate::social::time;

/// The mod log (§4.12). Pending while author-scope records need the author tier (§4.9).
///
/// An author-scope record missing from [`AuthorTier::status`] is left out of the log, so a
/// vector catches the omission.
pub fn mod_log(
    cx: &Cx<'_>,
    l1: &Moderation,
    author: &Stage<AuthorTier>,
) -> Stage<Vec<ModLogEntry>> {
    let mut out = Vec::new();
    for m in l1.records.values() {
        let status = if m.no_effect.is_some() {
            Some(ModStatus::ReservedUnknown)
        } else {
            match m.scope {
                ModScope::Forum => l1.forum_status.get(&m.uri).copied(),
                ModScope::Author => {
                    let author = author.as_ref().map_err(|p| *p)?;
                    author.status.get(&m.uri).copied()
                }
                ModScope::Unknown => Some(ModStatus::ReservedUnknown),
            }
        };
        if let Some(status) = status {
            out.push(ModLogEntry {
                uri: m.uri.clone(),
                status,
                time: m.time,
            });
        }
    }
    for x in cx.records() {
        if let SkelData::Revision {
            subject: Some(s),
            op: Some(op),
            ..
        } = &x.data
            && x.valid
            && cx.coll_of(&s.uri) == Some(Collection::ModAction)
        {
            let status = match op.as_str() {
                "update" => ModStatus::Edited,
                "delete" => ModStatus::Removed,
                _ => continue,
            };
            out.push(ModLogEntry {
                uri: x.uri.clone(),
                status,
                time: x.created_at.as_deref().and_then(time::us),
            });
        }
    }
    // (time desc, uri asc); no time last
    out.sort_by(|a, b| {
        a.time
            .is_none()
            .cmp(&b.time.is_none())
            .then_with(|| b.time.cmp(&a.time))
            .then_with(|| a.uri.cmp(&b.uri))
    });
    Ok(out)
}

/// The mod-queue items the fold provides (§4.12), sorted by (type, key bytes).
pub fn mod_queue(cx: &Cx<'_>, l1: &Moderation, l2: &Participation) -> Vec<QueueItem> {
    let mut out: Vec<QueueItem> = l2
        .applicants
        .iter()
        .map(|d| QueueItem::Applicant(d.clone()))
        .collect();
    out.extend(l1.conflicts.keys().map(|s| QueueItem::Conflict(s.clone())));
    for (uri, st) in &l1.forum_status {
        if *st == ModStatus::Stale
            && let Some(m) = l1.records.get(uri)
            && m.edited_subject(cx)
        {
            out.push(QueueItem::Stale(uri.clone()));
        }
    }
    out.sort();
    out
}

/// Raw-view annotations (§4.12).
pub fn raw(cx: &Cx<'_>, l1: &Moderation) -> Raw {
    let invalid = cx
        .records()
        .filter(|x| !x.valid)
        .map(|x| (x.uri.clone(), x.rule.clone().unwrap_or_default()))
        .collect();
    let mut assertions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in l1.records.values() {
        assertions
            .entry(m.subject.clone())
            .or_default()
            .push(m.uri.clone());
    }
    Raw {
        invalid,
        assertions,
    }
}
