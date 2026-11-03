// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::{BTreeMap, BTreeSet};

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::state::ForumState;
use crate::engine::{Engine, EngineError};
use crate::host::Host;
use crate::indexer::client::Client;
use crate::indexer::contract::{Change, MAX_CHANGES, PostHint, PostsPage, RepoHeadHint};
use crate::indexer::proofs::{ProofStatus, hydrate_visible};
use crate::social::full::view::State;
use crate::store::RepoStatus;
use crate::store::records::RecordStore;
use crate::sync::{ObservedChange, PointOutcome, Session};
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::tid::Tid;
use lessdugin_lexicon::collection::Collection;

pub use crate::dto::{AuditCheck, AuditCounts, AuditFinding, AuditKind, AuditReport};

const TWO_MINUTES_MS: i64 = 120_000;
const MAX_AUDIT_ITEMS: usize = 10_000;
/// Pages read from one paged listing, so a stream of empty pages cannot hold the audit.
const MAX_AUDIT_PAGES: usize = 1_000;
/// The first A2 window below the change log's head (rows); each further window doubles.
const A2_WINDOW: i64 = 5_000;

impl AuditCounts {
    fn count(&mut self, kind: AuditKind) {
        match kind {
            AuditKind::Contradiction => self.contradictions += 1,
            AuditKind::Omission => self.omissions += 1,
            AuditKind::Lag => self.lag += 1,
            AuditKind::Unchecked => self.unchecked += 1,
        }
    }
}

impl AuditCheck {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    fn checked(&mut self, n: usize) {
        self.counts.checked += n;
    }

    /// An item the audit could not check (its PDS or the proof endpoint was unreachable, or its
    /// proof is over the bundle cap): an `unchecked` finding with the reason, counted in
    /// `unchecked` and never as checked, a contradiction or an omission (`sync.md` §15.4).
    fn unchecked(&mut self, subject: &str, detail: impl std::fmt::Display) {
        tracing::warn!(check = %self.name, %subject, %detail, "indexer audit item unchecked");
        self.finding(AuditKind::Unchecked, subject, detail.to_string());
    }

    fn finding(&mut self, kind: AuditKind, subject: impl Into<String>, detail: impl Into<String>) {
        self.counts.count(kind);
        self.findings.push(AuditFinding {
            check: self.name.clone(),
            kind,
            subject: subject.into(),
            detail: detail.into(),
        });
    }
}

impl AuditReport {
    pub fn findings(&self) -> impl Iterator<Item = &AuditFinding> {
        self.checks.iter().flat_map(|c| c.findings.iter())
    }

    pub fn has_contradiction_or_omission(&self) -> bool {
        self.counts.contradictions > 0 || self.counts.omissions > 0
    }

    fn finish(&mut self) {
        for check in &self.checks {
            self.counts.checked += check.counts.checked;
            self.counts.contradictions += check.counts.contradictions;
            self.counts.omissions += check.counts.omissions;
            self.counts.lag += check.counts.lag;
            self.counts.unchecked += check.counts.unchecked;
        }
    }
}

fn engine_store(e: crate::store::StoreError) -> EngineError {
    EngineError::internal(format!("audit store: {e}"))
}

/// Shared A1–A5 implementation used by the CLI and, later, the service agent's settings action.
pub(crate) async fn run<H: Host>(
    engine: &Engine<H>,
    client: &Client,
    session: &Session<H>,
    key: &ForumKey,
) -> Result<AuditReport, EngineError> {
    let mut report = AuditReport {
        forum: key.uri.to_string(),
        indexer: client.did().to_owned(),
        endpoint: client.endpoint().to_string(),
        ..AuditReport::default()
    };
    let loaded = engine.loaded(key).await?;
    let state = ForumState::load(
        engine.host().cache(),
        key,
        &loaded.model.decl,
        engine.host().now_ms().saturating_mul(1000),
    )
    .await
    .map_err(engine_store)?;
    let dids: Vec<Did> = state
        .completeness_set()
        .iter()
        .filter_map(|d| Did::parse(d).ok())
        .collect();
    let store = RecordStore::new(engine.host().cache());

    // A1: remote heads are compared with real PDS observations in DID batches of 50.
    let mut a1 = AuditCheck::new("A1 heads");
    for chunk in dids.chunks(50) {
        let heads = client.get_repo_heads(session, chunk).await?;
        let by_did: BTreeMap<&str, &RepoHeadHint> =
            heads.heads.iter().map(|h| (h.did.as_str(), h)).collect();
        for did in chunk {
            // One deactivated or unreachable PDS leaves its repo unchecked, not the audit.
            if let Err(e) = engine.syncer().head_check(session, did, false).await {
                a1.unchecked(did.as_str(), e);
                continue;
            }
            a1.checked(1);
            let head = store
                .repo(did.as_str())
                .await
                .map_err(engine_store)?
                .and_then(|r| r.head_seen);
            let (Some(head), Some(remote)) = (head, by_did.get(did.as_str())) else {
                continue;
            };
            let Some(remote_rev) = remote.rev.as_deref() else {
                continue;
            };
            if remote_rev > head.rev.as_str() {
                a1.finding(
                    AuditKind::Contradiction,
                    did.as_str(),
                    format!("indexer rev {remote_rev} is above PDS head {}", head.rev),
                );
            } else if remote_rev < head.rev.as_str()
                && let (Ok(r), Ok(h)) = (Tid::parse(remote_rev), Tid::parse(&head.rev))
            {
                let behind_ms =
                    (h.timestamp_micros().saturating_sub(r.timestamp_micros()) / 1000) as i64;
                if behind_ms > heads.meta.as_of.lag_ms.saturating_add(TWO_MINUTES_MS) {
                    a1.finding(
                        AuditKind::Lag,
                        did.as_str(),
                        format!("indexer is {behind_ms} ms behind the PDS head"),
                    );
                }
            }
        }
    }
    report.checks.push(a1);

    // A2: only session Jetstream observations old enough for the indexer to have ingested.
    let mut a2 = AuditCheck::new("A2 changes");
    let now_us = engine.host().now_ms().saturating_mul(1000);
    let observed: Vec<_> = engine
        .syncer()
        .observed_changes()
        .into_iter()
        .filter(|c| c.fk == key.fk && now_us - c.observed_us > TWO_MINUTES_MS * 1000)
        .collect();
    if !observed.is_empty() {
        let scan = observed_rows(client, session, &key.uri, &observed).await?;
        for (i, old) in observed.iter().enumerate() {
            if scan.matched.contains(&i) {
                a2.checked(1);
            } else if scan.complete {
                a2.checked(1);
                a2.finding(
                    AuditKind::Omission,
                    &old.uri,
                    format!(
                        "Jetstream change at rev {} is missing from getChanges",
                        old.rev
                    ),
                );
            } else {
                a2.unchecked(&old.uri, "getChanges could not be read back far enough");
            }
        }
    }
    report.checks.push(a2);

    // A3/A4: remote order and tallies stay transport hints; the comparison values are local.
    let mut a3 = AuditCheck::new("A3 records");
    let mut a4 = AuditCheck::new("A4 digest and aggregates");
    let remote_posts = all_posts(client, session, &key.uri).await?;
    let remote_owned: BTreeSet<String> = remote_posts
        .iter()
        .filter_map(|p| p.record.at_uri())
        .map(|u| u.to_string())
        .collect();
    let complete_dids: BTreeSet<&str> = loaded
        .model
        .input
        .repo_status
        .iter()
        .filter(|(_, status)| **status == RepoStatus::Fresh)
        .map(|(did, _)| did.as_str())
        .collect();
    let now_ms = engine.host().now_ms();
    let eligible: Vec<_> = loaded
        .model
        .input
        .records
        .values()
        .filter(|r| {
            // The indexer lists what its fold lists: a hidden or removed post, or one by a banned
            // or nullified author, is valid but correctly absent from listPosts.
            r.valid
                && r.coll == Some(Collection::Post)
                && loaded.model.displayed(&r.uri).is_some_and(State::listed)
                && r.times
                    .obs
                    .is_some_and(|t| now_ms.saturating_mul(1000) - t > TWO_MINUTES_MS * 1000)
                && complete_dids.contains(r.did.as_str())
        })
        .collect();
    a3.checked(eligible.len());
    for post in &eligible {
        if !remote_owned.contains(&post.uri) {
            a3.finding(
                AuditKind::Omission,
                &post.uri,
                "eligible post on a complete repo is missing from listPosts(sort=new)",
            );
        }
    }
    let local_uris: BTreeSet<&str> = loaded
        .model
        .input
        .records
        .keys()
        .map(String::as_str)
        .collect();
    for uri in &remote_owned {
        if local_uris.contains(uri.as_str()) {
            a3.checked(1);
            continue;
        }
        let Ok(parsed) = AtUri::parse(uri) else {
            continue;
        };
        // A post of this forum is in its fk range (data-model.md §6.3). Anything else is a false
        // claim on its face, and is never fetched from a DID the indexer chose.
        if parsed.collection().as_str() != Collection::Post.nsid()
            || !crate::store::schema::in_range(&key.fk, parsed.rkey().as_str())
        {
            a3.checked(1);
            a3.finding(
                AuditKind::Contradiction,
                uri,
                "indexer lists a record that is not a post of this forum",
            );
            continue;
        }
        match engine.syncer().point_fetch(session, &parsed, None).await {
            Ok(PointOutcome::NotFound { .. }) => {
                a3.checked(1);
                a3.finding(
                    AuditKind::Contradiction,
                    uri,
                    "indexer lists a post that the author's PDS confirms absent",
                );
            }
            Ok(_) => a3.checked(1),
            Err(e) => a3.unchecked(uri, e),
        }
    }
    if !loaded.completeness.partial {
        let local_digest = &loaded.model.view.input_digest;
        if let Some(remote_digest) = remote_posts_digest(client, session, &key.uri).await? {
            a4.checked(1);
            if remote_digest != *local_digest {
                a4.finding(
                    AuditKind::Contradiction,
                    key.uri.to_string(),
                    format!("inputDigest differs: indexer {remote_digest}, local {local_digest}"),
                );
            }
        }
        if let Ok(scores) = &loaded.model.view.scores {
            for post in &remote_posts {
                let Some(tally) = &post.tally else { continue };
                let Some(local) = scores.get(&post.record.uri) else {
                    continue;
                };
                a4.checked(1);
                let unweighted = i64::from(local.unweighted);
                if tally.score != local.score
                    || tally.agreement != local.agreement
                    || tally.unweighted.unwrap_or(0) != unweighted
                {
                    a4.finding(
                        AuditKind::Contradiction,
                        &post.record.uri,
                        "tally differs from local recomputation",
                    );
                }
            }
        }
    }
    report.checks.push(a3);
    report.checks.push(a4);

    // A5: deterministic sample of up to 25 locally eligible post URIs. Stale is not misconduct.
    let mut a5 = AuditCheck::new("A5 proofs");
    let sample: Vec<AtUri> = eligible
        .iter()
        .take(25)
        .filter_map(|r| AtUri::parse(&r.uri).ok())
        .collect();
    if !sample.is_empty() {
        let proofs = hydrate_visible(&engine.syncer(), client, session, &sample).await?;
        for item in proofs.items {
            let error = item.error.as_deref().unwrap_or("proof item failed");
            match item.status {
                // Only a presented proof that fails proofs/1 contradicts the indexer.
                ProofStatus::Failed => {
                    a5.checked(1);
                    a5.finding(AuditKind::Contradiction, &item.uri, error);
                }
                // No proof was presented for a requested URI.
                ProofStatus::Missing => {
                    a5.checked(1);
                    a5.finding(AuditKind::Omission, &item.uri, error);
                }
                // A transport failure or an oversized single proof presents nothing to verify.
                ProofStatus::Unavailable | ProofStatus::TooLarge => a5.unchecked(&item.uri, error),
                ProofStatus::Admitted | ProofStatus::Absent | ProofStatus::StaleProof => {
                    a5.checked(1)
                }
            }
        }
    }
    report.checks.push(a5);
    report.finish();
    Ok(report)
}

/// A2 match (`sync.md` §15.4): a row for the same uri and DID at a later rev (a later state
/// supersedes the observed one; a row written from a snapshot carries the repo's head rev), or at
/// the observed rev with the observed CID (none for a delete).
fn change_matches(seen: &ObservedChange, row: &Change) -> bool {
    if row.uri != seen.uri || row.did != seen.did {
        return false;
    }
    let (Ok(observed), Ok(logged)) = (Tid::parse(&seen.rev), Tid::parse(&row.rev)) else {
        return false;
    };
    logged > observed || (logged == observed && row.cid == seen.cid)
}

/// Which observations `getChanges` holds a matching row for. `complete` is false when the scan
/// stopped before it could rule a change out (the page bound, or a cursor rejected at the floor).
struct ChangeScan {
    matched: BTreeSet<usize>,
    complete: bool,
}

/// Reads the forum's change log back from its head in doubling windows until every observation
/// is matched or the start of the retained log is reached. The log is kept 30 days (§15.2), so
/// reading from 0 is refused once it has been pruned, and its oldest rows are rarely the
/// session's; on `CursorTooOld` the scan restarts at the oldest cursor the indexer accepts.
async fn observed_rows<H: Host>(
    client: &Client,
    session: &Session<H>,
    forum: &AtUri,
    observed: &[ObservedChange],
) -> Result<ChangeScan, EngineError> {
    let mut by_uri: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, o) in observed.iter().enumerate() {
        by_uri.entry(o.uri.as_str()).or_default().push(i);
    }
    let mut scan = Scan {
        observed,
        by_uri,
        matched: BTreeSet::new(),
        pages: 0,
    };
    let head = client.current_as_of(session).await?.seq.max(0);
    let (mut width, mut hi, mut floor) = (A2_WINDOW, None, false);
    let mut lo = head.saturating_sub(width).max(0);
    loop {
        match scan.window(client, session, forum, lo, hi).await {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) if e.error_name() == Some("CursorTooOld") && !floor => {
                lo = oldest_accepted(client, session, forum, lo, hi.unwrap_or(head)).await?;
                floor = true;
                continue;
            }
            Err(e) if e.error_name() == Some("CursorTooOld") => break,
            Err(e) => return Err(e.into()),
        }
        if floor || lo == 0 || scan.matched.len() == observed.len() {
            return Ok(ChangeScan {
                matched: scan.matched,
                complete: true,
            });
        }
        hi = Some(lo);
        width = width.saturating_mul(2);
        lo = lo.saturating_sub(width).max(0);
    }
    Ok(ChangeScan {
        matched: scan.matched,
        complete: false,
    })
}

struct Scan<'a> {
    observed: &'a [ObservedChange],
    by_uri: BTreeMap<&'a str, Vec<usize>>,
    matched: BTreeSet<usize>,
    pages: usize,
}

impl Scan<'_> {
    /// Reads rows with `lo < seq` (and `seq <= hi` when bounded) until the cursor stops
    /// advancing. A short or empty page is not the end: the indexer drops blocklisted rows after
    /// paging. `false` when the page bound ended the read.
    async fn window<H: Host>(
        &mut self,
        client: &Client,
        session: &Session<H>,
        forum: &AtUri,
        lo: i64,
        hi: Option<i64>,
    ) -> Result<bool, crate::xrpc::XrpcCallError> {
        let mut since = lo;
        while hi.is_none_or(|h| since < h) {
            if self.pages >= MAX_AUDIT_PAGES {
                return Ok(false);
            }
            self.pages += 1;
            let page = client
                .get_changes(session, Some(forum), since, MAX_CHANGES as u32)
                .await?;
            for row in &page.changes {
                if row.seq <= since || hi.is_some_and(|h| row.seq > h) {
                    continue;
                }
                for &i in self.by_uri.get(row.uri.as_str()).into_iter().flatten() {
                    if change_matches(&self.observed[i], row) {
                        self.matched.insert(i);
                    }
                }
            }
            if page.cursor <= since {
                break;
            }
            since = page.cursor;
        }
        Ok(true)
    }
}

/// The smallest cursor in `(too_old, accepted]` that `getChanges` does not refuse as too old.
async fn oldest_accepted<H: Host>(
    client: &Client,
    session: &Session<H>,
    forum: &AtUri,
    mut too_old: i64,
    mut accepted: i64,
) -> Result<i64, EngineError> {
    while accepted - too_old > 1 {
        let mid = too_old + (accepted - too_old) / 2;
        match client.get_changes(session, Some(forum), mid, 1).await {
            Ok(_) => accepted = mid,
            Err(e) if e.error_name() == Some("CursorTooOld") => too_old = mid,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(accepted)
}

async fn all_posts<H: Host>(
    client: &Client,
    session: &Session<H>,
    forum: &AtUri,
) -> Result<Vec<PostHint>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = None;
    let mut seen = BTreeSet::new();
    // A short page is not the end: the indexer drops blocklisted rows after paging. Page until
    // the cursor is absent or repeats, within the item cap and a page bound.
    for _ in 0..MAX_AUDIT_PAGES {
        let page: PostsPage = client
            .list_posts(session, forum, "new", 100, cursor.as_deref())
            .await?;
        cursor = page.cursor;
        out.extend(page.posts);
        if out.len() >= MAX_AUDIT_ITEMS {
            break;
        }
        let Some(c) = cursor.clone() else { break };
        if !seen.insert(c) {
            break;
        }
    }
    out.truncate(MAX_AUDIT_ITEMS);
    Ok(out)
}

async fn remote_posts_digest<H: Host>(
    client: &Client,
    session: &Session<H>,
    forum: &AtUri,
) -> Result<Option<String>, EngineError> {
    Ok(Some(
        client
            .list_posts(session, forum, "new", 1, None)
            .await?
            .input_digest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchecked_item_is_a_finding_counted_apart_from_contradictions_and_omissions() {
        let mut a1 = AuditCheck::new("A1 heads");
        a1.checked(1);
        a1.unchecked("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa", "HTTP 503");
        let mut a3 = AuditCheck::new("A3 records");
        a3.unchecked("at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/x/y", "offline");
        let mut report = AuditReport {
            checks: vec![a1, a3],
            ..AuditReport::default()
        };
        report.finish();
        assert_eq!(
            report.counts,
            AuditCounts {
                checked: 1,
                unchecked: 2,
                ..AuditCounts::default()
            }
        );
        assert_eq!(report.checks[0].counts.unchecked, 1);
        let findings: Vec<_> = report.findings().collect();
        assert_eq!(findings.len(), 2);
        assert_eq!(
            findings[0],
            &AuditFinding {
                check: "A1 heads".into(),
                kind: AuditKind::Unchecked,
                subject: "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa".into(),
                detail: "HTTP 503".into(),
            }
        );
        // never a contradiction or an omission, so never exit code 6
        assert!(!report.has_contradiction_or_omission());
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["counts"]["unchecked"], 2);
        assert_eq!(json["checks"][1]["findings"][0]["kind"], "unchecked");
        // omitted when 0, and a report without it still reads
        let zero = serde_json::to_value(AuditCounts::default()).unwrap();
        assert!(zero.get("unchecked").is_none(), "{zero}");
        let v0: AuditCounts = serde_json::from_value(
            serde_json::json!({"checked": 1, "contradictions": 0, "omissions": 0, "lag": 0}),
        )
        .unwrap();
        assert_eq!(v0.unchecked, 0);
    }
}
