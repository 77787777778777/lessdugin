// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::{BTreeMap, BTreeSet};

use futures::future::join_all;
use lessdugin_atp::proof::{BundleCaps, ProofBundle, ProofCaps, ProofErrorKind, verify_proof};
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::xrpc::SyncGetRecord;
use serde::{Deserialize, Serialize};

use crate::host::Host;
use crate::identity::{IdentityLevel, THIRD_PARTY_DOC_MAX_AGE_MS};
use crate::store::admit::{self, LexiconValidator, Observation};
use crate::store::records::{Delete, DeleteEvidence, RecordStore};
use crate::store::repo::HeadSeen;
use crate::store::{OrderKey, Source};
use crate::sync::{Session, Syncer};
use crate::xrpc::XrpcCallError;

use super::client::Client;
use super::contract::MAX_PROOF_URIS;

const HEAD_CURRENCY_MS: i64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProofStatus {
    Admitted,
    Absent,
    StaleProof,
    /// The bundle presented a proof that failed `proofs/1` (§8.6): counted against the source.
    Failed,
    /// The bundle omitted this requested URI.
    Missing,
    /// `getRecordProofs` could not be fetched (transport failure, 5xx after retries): no proof
    /// was presented, so this says nothing about the indexer's honesty.
    Unavailable,
    /// This single URI's bundle is still over the cap; the record is fetched from the author's
    /// PDS instead (§8.6). Not a failure.
    TooLarge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofItemStatus {
    pub uri: String,
    pub status: ProofStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub point_fetched: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofReport {
    pub requested: usize,
    pub admitted: usize,
    pub absent: usize,
    pub stale: usize,
    pub failed: usize,
    pub missing: usize,
    #[serde(default)]
    pub unavailable: usize,
    #[serde(default)]
    pub too_large: usize,
    pub point_fetched: usize,
    pub items: Vec<ProofItemStatus>,
}

impl ProofReport {
    fn finish_counts(&mut self) {
        self.requested = self.items.len();
        self.admitted = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::Admitted)
            .count();
        self.absent = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::Absent)
            .count();
        self.stale = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::StaleProof)
            .count();
        self.failed = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::Failed)
            .count();
        self.missing = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::Missing)
            .count();
        self.unavailable = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::Unavailable)
            .count();
        self.too_large = self
            .items
            .iter()
            .filter(|i| i.status == ProofStatus::TooLarge)
            .count();
        self.point_fetched = self.items.iter().filter(|i| i.point_fetched).count();
    }

    pub fn proof_failures(&self) -> usize {
        self.failed + self.missing
    }
}

struct ItemResult {
    status: ProofItemStatus,
    fallback: bool,
}

fn invalid(message: impl Into<String>) -> XrpcCallError {
    XrpcCallError::InvalidRequest(message.into())
}

fn bundle_too_large(e: &XrpcCallError) -> bool {
    match e {
        XrpcCallError::TooLarge => true,
        XrpcCallError::Http(status) => *status == 413,
        XrpcCallError::Xrpc {
            status, message, ..
        } => {
            *status == 413
                || (*status == 500
                    && message.as_deref().is_some_and(|m| {
                        m.contains("proof bundle exceeds") || m.contains("response too large")
                    }))
        }
        _ => false,
    }
}

fn store_error(e: crate::store::StoreError) -> XrpcCallError {
    invalid(format!("proof record store: {e}"))
}

/// Hydrates visible URIs in independent bundles of at most 25. Cold-start callers pass their
/// visible page (normally 25, or two pages of 25), so no background fan-out is needed.
///
/// The bundle requests and one head check per author run at once, all at the session's
/// priority (P0 for a page on screen, `sync.md` §14.3 steps 3 and 4). Each author's DID document
/// is resolved first, at most 1 h old (§8.6), and then serves both the head check and the
/// verification of the author's proofs.
pub async fn hydrate_visible<H: Host>(
    syncer: &Syncer<H>,
    client: &Client,
    session: &Session<H>,
    uris: &[AtUri],
) -> Result<ProofReport, XrpcCallError> {
    let authors: BTreeSet<&Did> = uris.iter().map(AtUri::did).collect();
    let checks = join_all(authors.into_iter().map(|did| async move {
        // A failure is handled per item as non-current and triggers the point fetch, which
        // reports the useful identity or PDS error there.
        if session
            .resolver()
            .resolve_did_doc(did, THIRD_PARTY_DOC_MAX_AGE_MS)
            .await
            .is_ok()
        {
            let _ = syncer.head_check(session, did, false).await;
        }
    }));
    let chunks: Vec<&[AtUri]> = uris.chunks(MAX_PROOF_URIS).collect();
    let fetches = join_all(chunks.iter().map(|c| fetch_bundle(client, session, c)));
    let (_, bundles) = futures::join!(checks, fetches);
    let mut report = ProofReport::default();
    for (chunk, fetched) in chunks.into_iter().zip(bundles) {
        let part = hydrate_chunk(syncer, client, session, chunk, fetched).await?;
        report.items.extend(part.items);
    }
    report.finish_counts();
    Ok(report)
}

/// A `getRecordProofs` bundle and the time it was received.
async fn fetch_bundle<H: Host>(
    client: &Client,
    session: &Session<H>,
    uris: &[AtUri],
) -> Result<(Vec<u8>, i64), XrpcCallError> {
    let car = client.get_record_proofs(session, uris).await?;
    Ok((car, session.host().now_ms()))
}

/// Admits one bundle's items; the head checks of their authors have run.
async fn hydrate_chunk<H: Host>(
    syncer: &Syncer<H>,
    client: &Client,
    session: &Session<H>,
    uris: &[AtUri],
    fetched: Result<(Vec<u8>, i64), XrpcCallError>,
) -> Result<ProofReport, XrpcCallError> {
    let (car, received_at) = match fetched {
        Ok(f) => f,
        Err(e) if bundle_too_large(&e) && uris.len() > 1 => {
            // Split into ordered, strictly smaller batches. The one-URI base case below makes
            // this recursion finite even if a service rejects every request as oversized.
            let (l, r) = uris.split_at(uris.len() / 2);
            let fetched = fetch_bundle(client, session, l).await;
            let mut left = Box::pin(hydrate_chunk(syncer, client, session, l, fetched)).await?;
            let fetched = fetch_bundle(client, session, r).await;
            let right = Box::pin(hydrate_chunk(syncer, client, session, r, fetched)).await?;
            left.items.extend(right.items);
            left.finish_counts();
            return Ok(left);
        }
        Err(e) if bundle_too_large(&e) => {
            // An individually oversized proof is not evidence of indexer misconduct. Fetch the
            // record from its author's PDS instead, as required by sync.md §8.6.
            let mut item = ProofItemStatus {
                uri: uris[0].to_string(),
                status: ProofStatus::TooLarge,
                rev: None,
                error: Some(format!("proof bundle exceeds its cap: {e}")),
                point_fetched: false,
            };
            point_fallback(syncer, session, &uris[0], &mut item).await;
            let mut report = ProofReport {
                items: vec![item],
                ..ProofReport::default()
            };
            report.finish_counts();
            return Ok(report);
        }
        Err(e) => {
            // A failed bundle request says nothing about the author's PDS. Keep the source error
            // visible, but converge each requested record through ordinary PDS admission.
            let mut report = ProofReport::default();
            for uri in uris {
                let mut item = ProofItemStatus {
                    uri: uri.to_string(),
                    status: ProofStatus::Unavailable,
                    rev: None,
                    error: Some(format!("indexer proof request failed: {e}")),
                    point_fetched: false,
                };
                point_fallback(syncer, session, uri, &mut item).await;
                report.items.push(item);
            }
            report.finish_counts();
            return Ok(report);
        }
    };
    let origin = client.origin();
    let store = RecordStore::new(session.host().cache());
    let bundle = match ProofBundle::parse(&car, &BundleCaps::default()) {
        Ok(b) => b,
        Err(e) => {
            strike_host(syncer, session, &origin, uris.len() as u32).await?;
            let mut report = ProofReport::default();
            for uri in uris {
                let mut item = ProofItemStatus {
                    uri: uri.to_string(),
                    status: ProofStatus::Failed,
                    rev: None,
                    error: Some(e.to_string()),
                    point_fetched: false,
                };
                point_fallback(syncer, session, uri, &mut item).await;
                report.items.push(item);
            }
            report.finish_counts();
            return Ok(report);
        }
    };

    let mut by_uri: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, item) in bundle.items.iter().enumerate() {
        if let Ok(item) = item {
            by_uri.entry(item.uri.as_str()).or_default().push(i);
        }
    }
    let requested: BTreeSet<&str> = uris.iter().map(AtUri::as_str).collect();
    let extra = bundle
        .items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter(|i| !requested.contains(i.uri.as_str()))
        .count();
    let duplicate = by_uri.values().filter(|v| v.len() > 1).count();
    if extra + duplicate > 0 {
        strike_host(syncer, session, &origin, (extra + duplicate) as u32).await?;
    }

    let mut results = Vec::with_capacity(uris.len());
    let mut strikes = 0u32;
    for uri in uris {
        let indices = by_uri.get(uri.as_str()).cloned().unwrap_or_default();
        if indices.len() != 1 {
            let missing = indices.is_empty();
            let result = ItemResult {
                status: ProofItemStatus {
                    uri: uri.to_string(),
                    status: if missing {
                        ProofStatus::Missing
                    } else {
                        ProofStatus::Failed
                    },
                    rev: None,
                    error: Some(if missing {
                        "the bundle omitted this requested URI".to_owned()
                    } else {
                        "the bundle repeated this requested URI".to_owned()
                    }),
                    point_fetched: false,
                },
                fallback: true,
            };
            strikes += 1;
            results.push(result);
            continue;
        }
        let index = indices[0];
        let mut ident = match session
            .resolver()
            .resolve_did_doc(uri.did(), THIRD_PARTY_DOC_MAX_AGE_MS)
            .await
        {
            Ok(i) if i.level == IdentityLevel::Doc => i,
            Ok(_) => {
                strikes += 1;
                results.push(failed(uri, "proof needs a document-level identity"));
                continue;
            }
            Err(e) => {
                // Identity/transport failure is not evidence of indexer misconduct. It still
                // cannot become content, so use the ordinary PDS fallback.
                results.push(ItemResult {
                    status: ProofItemStatus {
                        uri: uri.to_string(),
                        status: ProofStatus::StaleProof,
                        rev: None,
                        error: Some(format!("could not establish proof currency: {e}")),
                        point_fetched: false,
                    },
                    fallback: true,
                });
                continue;
            }
        };
        let Some(mut key) = ident.public_key() else {
            strikes += 1;
            results.push(failed(
                uri,
                "the DID document has no supported #atproto key",
            ));
            continue;
        };
        let current_head = latest_head(syncer, &store, uri.did())
            .await?
            .filter(|h| h.sent_at >= received_at - HEAD_CURRENCY_MS);
        let now_us = session.host().now_ms().saturating_mul(1000);
        let mut verified = bundle.verify_item(index, &key, now_us);
        if verified
            .as_ref()
            .is_err_and(|e| e.kind() == ProofErrorKind::BadSignature)
            && ident.doc_at_ms < received_at
        {
            // The key may have rotated since the document was fetched: once more against a
            // document fetched now, before a bad signature counts (§8.6, as §15.1 does).
            if let Ok(fresh) = session.resolver().resolve_did_doc(uri.did(), 0).await
                && fresh.level == IdentityLevel::Doc
                && let Some(k) = fresh.public_key()
            {
                key = k;
                ident = fresh;
                verified = bundle.verify_item(index, &key, now_us);
            }
        }
        let outcome = match verified {
            Ok(o) => o,
            Err(e) => {
                if e.kind() == ProofErrorKind::BadSignature
                    && let (Some(rev), Some(head)) = (e.rev, &current_head)
                    && rev.as_str() < head.rev.as_str()
                {
                    let mut item = ProofItemStatus {
                        uri: uri.to_string(),
                        status: ProofStatus::StaleProof,
                        rev: Some(rev.to_string()),
                        error: Some("stale-proof: fetching a fresh author-PDS proof".into()),
                        point_fetched: false,
                    };
                    pds_proof_fallback(syncer, session, uri, &ident, &key, &mut item).await;
                    results.push(ItemResult {
                        status: item,
                        fallback: false,
                    });
                } else {
                    strikes += 1;
                    results.push(failed(uri, e.to_string()));
                }
                continue;
            }
        };
        let rev = outcome.rev.to_string();
        let head = latest_head(syncer, &store, uri.did()).await?.filter(|h| {
            rev.as_str() >= h.rev.as_str() && h.sent_at >= received_at - HEAD_CURRENCY_MS
        });
        let Some(head) = head else {
            results.push(ItemResult {
                status: ProofItemStatus {
                    uri: uri.to_string(),
                    status: ProofStatus::StaleProof,
                    rev: Some(rev),
                    error: Some("proof is not current at the author's PDS head".to_owned()),
                    point_fetched: false,
                },
                fallback: true,
            });
            continue;
        };
        let mut batch = store.batch();
        let write = if let Some(record) = outcome.record {
            match admit::admit_block(uri, &record.cid, &record.bytes, &LexiconValidator) {
                Ok(rec) => {
                    let mut obs = Observation::proof(
                        &rev,
                        head.sent_at,
                        Source::Indexer(client.did().to_owned()),
                        session.host().now_ms(),
                    );
                    if let Some(first) =
                        syncer.hint_first_seen(uri.as_str(), &record.cid.to_string())
                    {
                        obs = obs.with_first_seen(first);
                    }
                    batch
                        .put_version(rec.into_version(obs))
                        .await
                        .map(|_| ProofStatus::Admitted)
                        .map_err(store_error)
                }
                Err(e) => Err(invalid(format!("proven record admission: {e}"))),
            }
        } else {
            batch
                .tombstone(
                    uri,
                    Delete {
                        key: OrderKey::new(&rev, head.sent_at),
                        evidence: DeleteEvidence::RecordNotFound,
                        at_ms: session.host().now_ms(),
                    },
                )
                .await
                .map(|_| ProofStatus::Absent)
                .map_err(store_error)
        };
        match write {
            Ok(status) => {
                batch.commit().await.map_err(store_error)?;
                results.push(ItemResult {
                    status: ProofItemStatus {
                        uri: uri.to_string(),
                        status,
                        rev: Some(rev),
                        error: None,
                        point_fetched: false,
                    },
                    fallback: false,
                });
            }
            Err(e) => {
                strikes += 1;
                results.push(failed(uri, e.to_string()));
            }
        }
    }
    if strikes > 0 {
        strike_host(syncer, session, &origin, strikes).await?;
    }

    let mut report = ProofReport::default();
    for (mut result, uri) in results.into_iter().zip(uris) {
        if result.fallback {
            point_fallback(syncer, session, uri, &mut result.status).await;
        }
        report.items.push(result.status);
    }
    report.finish_counts();
    Ok(report)
}

/// The latest head observation of `did`: its stored `headSeen`, or one a head check handed to
/// a sync in flight that has not recorded it yet (`sync.md` §10.6).
async fn latest_head<H: Host>(
    syncer: &Syncer<H>,
    store: &RecordStore<'_, H::Kv>,
    did: &Did,
) -> Result<Option<HeadSeen>, XrpcCallError> {
    let stored = store
        .repo(did.as_str())
        .await
        .map_err(store_error)?
        .and_then(|r| r.head_seen);
    Ok(match (stored, syncer.pending_head(did.as_str())) {
        (Some(a), Some(b)) if b.sent_at > a.sent_at => Some(b),
        (a, b) => a.or(b),
    })
}

fn failed(uri: &AtUri, error: impl ToString) -> ItemResult {
    ItemResult {
        status: ProofItemStatus {
            uri: uri.to_string(),
            status: ProofStatus::Failed,
            rev: None,
            error: Some(error.to_string()),
            point_fetched: false,
        },
        fallback: true,
    }
}

/// A proof signed before a key rotation is not indexer misconduct. Recover from the author's
/// own PDS with a fresh `sync.getRecord`, retaining proof-level admission rather than lowering
/// the result to an unproven JSON fetch (`sync.md` §8.7).
async fn pds_proof_fallback<H: Host>(
    syncer: &Syncer<H>,
    session: &Session<H>,
    uri: &AtUri,
    ident: &crate::identity::ResolvedIdentity,
    key: &lessdugin_atp::crypto::PublicKey,
    item: &mut ProofItemStatus,
) {
    let result = async {
        let pds = ident
            .pds_url()
            .ok_or_else(|| invalid("the DID document has no usable PDS"))?;
        let before = session.host().now_ms();
        let response = session
            .xrpc()
            .query_raw(
                &crate::xrpc::Service::Anon(pds),
                &SyncGetRecord {
                    did: uri.did().clone(),
                    collection: uri.collection().clone(),
                    rkey: uri.rkey().clone(),
                },
                &crate::xrpc::CallOpts::capped(crate::limits::PROOF_CAR_MAX_BYTES)
                    .with_timeout_ms(crate::limits::CAR_TIMEOUT_MS),
            )
            .await?;
        let sent_at = crate::sync::net::sent_at(&response).unwrap_or(before);
        let outcome = verify_proof(
            &response.body,
            uri.did(),
            uri.collection(),
            uri.rkey(),
            key,
            session.host().now_ms().saturating_mul(1000),
            &ProofCaps::default(),
        )
        .map_err(|e| invalid(format!("fresh PDS proof failed: {e}")))?;
        let rev = outcome.rev.to_string();
        let store = RecordStore::new(session.host().cache());
        let _guard = syncer.net().repo_lock(uri.did()).await;
        let mut batch = store.batch();
        if let Some(record) = outcome.record {
            let admitted = admit::admit_block(uri, &record.cid, &record.bytes, &LexiconValidator)
                .map_err(|e| invalid(format!("fresh PDS proof admission: {e}")))?;
            let mut observation =
                Observation::proof(&rev, sent_at, Source::Proof, session.host().now_ms());
            if let Some(first) = syncer.hint_first_seen(uri.as_str(), &record.cid.to_string()) {
                observation = observation.with_first_seen(first);
            }
            batch
                .put_version(admitted.into_version(observation))
                .await
                .map_err(store_error)?;
        } else {
            batch
                .tombstone(
                    uri,
                    Delete {
                        key: OrderKey::new(&rev, sent_at),
                        evidence: DeleteEvidence::RecordNotFound,
                        at_ms: session.host().now_ms(),
                    },
                )
                .await
                .map_err(store_error)?;
        }
        batch.commit().await.map_err(store_error)?;
        Ok::<_, XrpcCallError>(rev)
    }
    .await;
    match result {
        Ok(rev) => {
            item.rev = Some(rev);
            item.error = None;
            item.point_fetched = true;
        }
        Err(e) => {
            item.error = Some(format!("stale-proof PDS recovery failed: {e}"));
        }
    }
}

async fn point_fallback<H: Host>(
    syncer: &Syncer<H>,
    session: &Session<H>,
    uri: &AtUri,
    item: &mut ProofItemStatus,
) {
    match syncer.point_fetch(session, uri, None).await {
        Ok(_) => item.point_fetched = true,
        Err(e) => {
            item.error = Some(format!(
                "{}; PDS fallback failed: {e}",
                item.error.as_deref().unwrap_or("proof unavailable")
            ));
        }
    }
}

async fn strike_host<H: Host>(
    syncer: &Syncer<H>,
    session: &Session<H>,
    origin: &str,
    strikes: u32,
) -> Result<(), XrpcCallError> {
    if strikes == 0 {
        return Ok(());
    }
    let store = RecordStore::new(session.host().cache());
    let mut batch = store.batch();
    let mut row = batch
        .host_caps(origin)
        .await
        .map_err(store_error)?
        .unwrap_or_default();
    row.strikes = row.strikes.saturating_add(strikes);
    batch.put_host_caps(origin, &row).map_err(store_error)?;
    batch.commit().await.map_err(store_error)?;
    // One breaker failure per bundle, not one per item (R4).
    syncer.net().strike(origin, session.host().now_ms());
    Ok(())
}
