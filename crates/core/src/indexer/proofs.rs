// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::proof::{BundleCaps, ProofBundle};
use lessdugin_atp::syntax::{AtUri, Did};
use serde::{Deserialize, Serialize};

use crate::host::Host;
use crate::identity::IdentityLevel;
use crate::store::admit::{self, LexiconValidator, Observation};
use crate::store::records::{Delete, DeleteEvidence, RecordStore};
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
    Failed,
    Missing,
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

fn store_error(e: crate::store::StoreError) -> XrpcCallError {
    invalid(format!("proof record store: {e}"))
}

/// Hydrates visible URIs in independent bundles of at most 25. Cold-start callers pass their
/// visible page (normally 25, or two pages of 25), so no background fan-out is needed.
pub async fn hydrate_visible<H: Host>(
    syncer: &Syncer<H>,
    client: &Client,
    session: &Session<H>,
    uris: &[AtUri],
) -> Result<ProofReport, XrpcCallError> {
    let mut report = ProofReport::default();
    for chunk in uris.chunks(MAX_PROOF_URIS) {
        let part = hydrate_chunk(syncer, client, session, chunk).await?;
        report.items.extend(part.items);
    }
    report.finish_counts();
    Ok(report)
}

async fn hydrate_chunk<H: Host>(
    syncer: &Syncer<H>,
    client: &Client,
    session: &Session<H>,
    uris: &[AtUri],
) -> Result<ProofReport, XrpcCallError> {
    let car = client.get_record_proofs(session, uris).await?;
    let received_at = session.host().now_ms();
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

    // Check every visible author's PDS head, independently of indexer getRepoHeads.
    let dids: BTreeSet<Did> = uris.iter().map(|u| u.did().clone()).collect();
    for did in &dids {
        // A failed head check is handled per item as non-current and triggers point fetch. The
        // point fetch reports the useful identity/PDS error there.
        let _ = syncer.head_check(session, did, false).await;
    }

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
        let ident = match session.resolver().resolve_did_fresh(uri.did(), 0).await {
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
        let Some(key) = ident.public_key() else {
            strikes += 1;
            results.push(failed(
                uri,
                "the DID document has no supported #atproto key",
            ));
            continue;
        };
        let outcome =
            match bundle.verify_item(index, &key, session.host().now_ms().saturating_mul(1000)) {
                Ok(o) => o,
                Err(e) => {
                    strikes += 1;
                    results.push(failed(uri, e.to_string()));
                    continue;
                }
            };
        let rev = outcome.rev.to_string();
        let head = store
            .repo(uri.did().as_str())
            .await
            .map_err(store_error)?
            .and_then(|r| r.head_seen)
            .filter(|h| {
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
