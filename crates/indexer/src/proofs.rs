// SPDX-License-Identifier: AGPL-3.0-or-later
//! `proofs/1` acquisition and bounded CAR bundles. Only proofs verified against a freshly
//! resolved DID document enter `proof_cache`; failed bundle items are omitted, never substituted.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use anyhow::{Context, anyhow};
use lessdugin_atp::car::{CarLimits, read_car, write_car};
use lessdugin_atp::cid;
use lessdugin_atp::proof::{ProofBundle, ProofCaps, verify_proof};
use lessdugin_atp::syntax::AtUri;
use lessdugin_atp::xrpc::SyncGetRecord;
use lessdugin_atp::{Cid, Ipld};
use lessdugin_core::limits;
use lessdugin_core::xrpc::{CallOpts, Service};

use crate::Indexer;
use crate::store::ProofRow;

pub async fn fetch(indexer: &Indexer, uri: &str) -> anyhow::Result<ProofRow> {
    let uri = AtUri::parse(uri).context("invalid proof URI")?;
    if indexer.blocked(uri.as_str()) {
        return Err(anyhow!("blocked"));
    }
    let ident = indexer
        .engine
        .resolver()
        .resolve_did_fresh(uri.did(), 0)
        .await?;
    let pds = ident
        .pds_url()
        .ok_or_else(|| anyhow!("{} has no usable PDS", uri.did()))?;
    let key = ident
        .public_key()
        .ok_or_else(|| anyhow!("{} has no usable signing key", uri.did()))?;
    let params = SyncGetRecord {
        did: uri.did().clone(),
        collection: uri.collection().clone(),
        rkey: uri.rkey().clone(),
    };
    let opts =
        CallOpts::capped(limits::PROOF_CAR_MAX_BYTES).with_timeout_ms(limits::CAR_TIMEOUT_MS);
    let result = async {
        let car = indexer
            .engine
            .xrpc()
            .query_raw(&Service::Anon(pds), &params, &opts)
            .await?
            .body;
        let out = verify_proof(
            &car,
            uri.did(),
            uri.collection(),
            uri.rkey(),
            &key,
            indexer.now_ms().saturating_mul(1000),
            &ProofCaps::default(),
        )
        .map_err(|e| anyhow!("proof verification failed: {e}"))?;
        Ok(ProofRow {
            uri: uri.to_string(),
            rev: out.rev.to_string(),
            car,
            commit: out.commit_cid.to_string(),
            cid: out.record_cid().map(ToString::to_string),
        })
    }
    .await;
    match result {
        Ok(row) => {
            indexer.store.put_proof(&row, indexer.now_ms())?;
            indexer
                .metrics
                .proofs_fetched
                .fetch_add(1, Ordering::Relaxed);
            Ok(row)
        }
        Err(e) => {
            indexer
                .metrics
                .proofs_failed
                .fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Builds a manifest-rooted bundle. Requested records whose proof cannot be fetched or verified
/// are dropped as required by data-model.md §11.3.
pub async fn bundle(indexer: &Indexer, uris: &[String]) -> anyhow::Result<Vec<u8>> {
    let mut items = Vec::new();
    let mut blocks: BTreeMap<Cid, Vec<u8>> = BTreeMap::new();
    for uri in uris.iter().take(25) {
        if indexer.blocked(uri) {
            continue;
        }
        let row = match indexer.store.proof(uri) {
            Some(row) => row,
            None => match fetch(indexer, uri).await {
                Ok(row) => row,
                Err(_) => continue,
            },
        };
        let car = match read_car(&row.car, &CarLimits::PROOF) {
            Ok(car) => car,
            Err(_) => {
                indexer
                    .metrics
                    .proofs_failed
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let commit = match cid::parse(&row.commit) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let record_cid = match row.cid.as_deref().map(cid::parse).transpose() {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (c, b) in car.blocks {
            blocks.insert(c, b);
        }
        let mut item = BTreeMap::new();
        item.insert("uri".into(), Ipld::String(row.uri));
        item.insert("cid".into(), record_cid.map_or(Ipld::Null, Ipld::Link));
        item.insert("commit".into(), Ipld::Link(commit));
        items.push(Ipld::Map(item));
    }
    let mut manifest = BTreeMap::new();
    manifest.insert("v".into(), Ipld::Integer(1));
    manifest.insert("items".into(), Ipld::List(items));
    let manifest = lessdugin_atp::dagcbor::encode(&Ipld::Map(manifest))?;
    let root = cid::for_dag_cbor(&manifest);
    let total = blocks.len() + 1;
    if total > CarLimits::BUNDLE.max_blocks {
        return Err(anyhow!("proof bundle exceeds block cap"));
    }
    let all = std::iter::once((root, manifest))
        .chain(blocks)
        .collect::<Vec<_>>();
    let bytes = write_car(&[root], all.iter().map(|(c, b)| (c, b.as_slice())));
    if bytes.len() > CarLimits::BUNDLE.max_total_bytes {
        return Err(anyhow!("proof bundle exceeds byte cap"));
    }
    // Parse our own output so a malformed manifest can never cross the HTTP boundary.
    ProofBundle::parse(&bytes, &Default::default())
        .map_err(|e| anyhow!("invalid generated bundle: {e}"))?;
    Ok(bytes)
}
