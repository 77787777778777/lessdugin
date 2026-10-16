// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin verify <ref> [--deep]` (`sync.md` §8.8): proof verification orchestration
//! (`proofs/1`).
//!
//! ```text
//! verify(uri, deep) -> VerifyReport
//!   doc  = DID document of did(uri), fetched now, level doc         → PDS endpoint, #atproto key
//!   car  = com.atproto.sync.getRecord at the PDS                     (Anon, no redirects, 1 MB)
//!   lessdugin_atp::proof::verify_proof(car, did, coll, rkey, key, now)   (= verify_record_proof)
//!   json = com.atproto.repo.getRecord at the PDS: cid(DAG-CBOR(json.value)) must equal the proven
//!          CID (or the PDS must report RecordNotFound for a proven absence)
//!   deep = strongRef and blob checks: not implemented in M1 (reported as a skipped check:
//!          ok, with detail DEEP_SKIPPED)
//! ```
//!
//! The result is `proven`/`absent` only if every check passed (the informational `deep` check
//! aside). Transport failures (network, timeout, 5xx, rate limits) are returned as errors rather
//! than reports, since nothing was learned about the record. Verification never writes the store
//! in M1 (raising a stored version's level to `proven` comes with the record store).

use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::proof::{ProofCaps, ProofError, verify_proof};
use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, DidMethod};
use lessdugin_atp::xrpc::{GetRecord, SyncGetRecord};
use lessdugin_atp::{Cid, commit::CommitError};

use crate::dto::{DtoVersion, VerifyCheck, VerifyOutcome, VerifyReport};
use crate::engine::EngineError;
use crate::host::Host;
use crate::identity::{IdentityError, IdentityLevel, Resolver, doc::curve_name};
use crate::limits;
use crate::xrpc::{CallOpts, Service, Xrpc, XrpcCallError};

/// Check names, in report order.
pub mod checks {
    /// The DID document was fetched now and passed the checks (level `doc`).
    pub const DID_DOCUMENT: &str = "did document";
    /// The document has an acceptable `#atproto_pds` endpoint.
    pub const PDS_ENDPOINT: &str = "pds endpoint";
    /// The document has a usable `#atproto` key (`keySource`).
    pub const SIGNING_KEY: &str = "signing key";
    /// `com.atproto.sync.getRecord` returned a proof CAR.
    pub const PROOF_FETCHED: &str = "proof fetched";
    /// The CAR parsed within caps, every block matches its CID, the root is a commit block.
    pub const CAR: &str = "car";
    /// The commit decodes, has version 3 and the repo's DID.
    pub const COMMIT: &str = "commit";
    /// The commit rev is at most 5 minutes in the future.
    pub const REV_NOT_IN_FUTURE: &str = "rev not in future";
    /// The commit signature verifies (low-S) with the `#atproto` key.
    pub const COMMIT_SIGNATURE: &str = "commit signature";
    /// The MST walk to `collection/rkey` is valid (presence or absence).
    pub const MST_PATH: &str = "mst path";
    /// The record block is in the CAR and hashes to the CID in the MST.
    pub const RECORD_CID: &str = "record cid";
    /// `com.atproto.repo.getRecord` agrees: its JSON hashes to the proven CID (or the PDS reports
    /// `RecordNotFound` for a proven absence).
    pub const RECORD_JSON: &str = "record json";
    /// `--deep` (informational in M1: reported as skipped, see [`super::DEEP_SKIPPED`]).
    pub const DEEP: &str = "deep";
}

/// The detail of the `deep` check while reference and blob checks are not implemented (M1). The
/// check is `ok` (it did not fail); clients show it as skipped.
pub const DEEP_SKIPPED: &str = "skipped: strongRef and blob checks are not implemented yet";

/// The proof-stage checks in the order `verify_proof` performs them.
const PROOF_STAGES: [&str; 6] = [
    checks::CAR,
    checks::COMMIT,
    checks::REV_NOT_IN_FUTURE,
    checks::COMMIT_SIGNATURE,
    checks::MST_PATH,
    checks::RECORD_CID,
];

fn failing_stage(e: &ProofError) -> usize {
    match e {
        ProofError::Commit(CommitError::FutureRev(_)) => 2,
        ProofError::Commit(CommitError::Signature(_)) => 3,
        ProofError::Commit(_) => 1,
        ProofError::Mst(_) => 4,
        ProofError::MissingRecord(_)
        | ProofError::BadRecord { .. }
        | ProofError::RecordMismatch { .. } => 5,
        // The CAR itself: parse, caps, block hashes, roots, the commit block.
        _ => 0,
    }
}

/// `"<plc|web> #atproto <k256|p256>"`.
pub fn key_source(did: &Did, key: &PublicKey) -> String {
    let method = match did.method_kind() {
        DidMethod::Plc => "plc",
        DidMethod::Web => "web",
        DidMethod::Other => did.method(),
    };
    format!("{method} #atproto {}", curve_name(key))
}

struct Report {
    report: VerifyReport,
}

impl Report {
    fn new(uri: &AtUri) -> Self {
        Self {
            report: VerifyReport {
                dto: DtoVersion::default(),
                uri: uri.to_string(),
                result: VerifyOutcome::Failed,
                rev: None,
                commit_cid: None,
                key_source: "unavailable".to_owned(),
                checks: Vec::new(),
            },
        }
    }

    fn check(&mut self, name: &str, ok: bool, detail: Option<String>) {
        self.report.checks.push(VerifyCheck {
            name: name.to_owned(),
            ok,
            detail,
        });
    }

    fn ok(&mut self, name: &str, detail: impl Into<String>) {
        self.check(name, true, Some(detail.into()));
    }

    fn fail(&mut self, name: &str, detail: impl Into<String>) -> VerifyReport {
        self.check(name, false, Some(detail.into()));
        self.report.clone()
    }
}

/// Verifies the record `uri` against its author's PDS (`sync.md` §8.8).
pub async fn verify_record<H: Host>(
    resolver: &Resolver<H>,
    uri: &AtUri,
    deep: bool,
) -> Result<VerifyReport, EngineError> {
    let host = resolver.host();
    let xrpc = Xrpc::new(host);
    let did = uri.did();
    let mut r = Report::new(uri);

    // 1. A DID document fetched now, at level doc.
    let ident = match resolver.resolve_did_fresh(did, 0).await {
        Ok(i) => i,
        Err(IdentityError::Fetch { error, .. }) if error.is_transient() => {
            return Err(EngineError::from(error).context(&format!("DID document of {did}")));
        }
        Err(e @ IdentityError::UnsupportedDid { .. }) => return Err(e.into()),
        Err(e) => return Ok(r.fail(checks::DID_DOCUMENT, e.to_string())),
    };
    if ident.level != IdentityLevel::Doc {
        return Ok(r.fail(
            checks::DID_DOCUMENT,
            "only a hint identity is available; proofs need the DID document",
        ));
    }
    r.ok(
        checks::DID_DOCUMENT,
        format!("{:?} document", ident.source).to_lowercase(),
    );

    // 2. PDS and key.
    let Some(pds) = ident.pds_url() else {
        let why = ident
            .pds_problem
            .clone()
            .unwrap_or_else(|| "no PDS".to_owned());
        return Ok(r.fail(checks::PDS_ENDPOINT, why));
    };
    r.ok(checks::PDS_ENDPOINT, ident.pds.clone().unwrap_or_default());
    let Some(key) = ident.public_key() else {
        let why = ident
            .key_problem
            .clone()
            .unwrap_or_else(|| "no key".to_owned());
        return Ok(r.fail(checks::SIGNING_KEY, format!("UnsupportedKey: {why}")));
    };
    r.report.key_source = key_source(did, &key);
    r.ok(checks::SIGNING_KEY, r.report.key_source.clone());

    // 3. The proof CAR.
    let service = Service::Anon(pds);
    let params = SyncGetRecord {
        did: did.clone(),
        collection: uri.collection().clone(),
        rkey: uri.rkey().clone(),
    };
    let opts =
        CallOpts::capped(limits::PROOF_CAR_MAX_BYTES).with_timeout_ms(limits::CAR_TIMEOUT_MS);
    let car = match xrpc.query_raw(&service, &params, &opts).await {
        Ok(resp) => resp.body,
        Err(e) if e.is_transient() => return Err(EngineError::from(e).context("sync.getRecord")),
        Err(e) => return Ok(r.fail(checks::PROOF_FETCHED, e.to_string())),
    };
    r.ok(checks::PROOF_FETCHED, format!("{} bytes", car.len()));

    // 4. proofs/1.
    let now_us = host.now_ms().saturating_mul(1000);
    let outcome = match verify_proof(
        &car,
        did,
        uri.collection(),
        uri.rkey(),
        &key,
        now_us,
        &ProofCaps::default(),
    ) {
        Ok(o) => o,
        Err(f) => {
            r.report.rev = f.rev.map(|t| t.to_string());
            r.report.commit_cid = f.commit_cid.map(|c| c.to_string());
            let stage = failing_stage(&f.error);
            for name in &PROOF_STAGES[..stage] {
                r.check(name, true, None);
            }
            // The detail starts with the proofs/1 error kind (`sync.md` §8.1), e.g. BadSignature.
            let detail = format!("{:?}: {}", f.kind(), f.error);
            return Ok(r.fail(PROOF_STAGES[stage], detail));
        }
    };
    r.report.rev = Some(outcome.rev.to_string());
    r.report.commit_cid = Some(outcome.commit_cid.to_string());
    r.ok(checks::CAR, format!("commit {}", outcome.commit_cid));
    r.ok(checks::COMMIT, "version 3, DID matches");
    r.ok(checks::REV_NOT_IN_FUTURE, format!("rev {}", outcome.rev));
    r.ok(checks::COMMIT_SIGNATURE, r.report.key_source.clone());
    let proven: Option<Cid> = outcome.record_cid().copied();
    match proven {
        Some(cid) => {
            r.ok(checks::MST_PATH, "present");
            r.ok(checks::RECORD_CID, cid.to_string());
        }
        None => r.ok(checks::MST_PATH, "absent"),
    }

    // 5. The JSON view agrees with the signed record.
    let params = GetRecord {
        repo: AtIdentifier::Did(did.clone()),
        collection: uri.collection().clone(),
        rkey: uri.rkey().clone(),
        cid: None,
    };
    let opts = CallOpts::capped(limits::RECORD_RESPONSE_MAX_BYTES);
    let json = xrpc.query_endpoint(&service, &params, &opts).await;
    let (ok, detail) = match (proven, json) {
        (_, Err(e)) if e.is_transient() => {
            return Err(EngineError::from(e).context("repo.getRecord"));
        }
        (Some(cid), Ok(out)) => match lessdugin_atp::datamodel::record_cid(&out.value) {
            Ok(c) if c != cid => (
                false,
                format!("the JSON value hashes to {c}, the signed record is {cid}"),
            ),
            Ok(_) if out.cid.as_deref().is_some_and(|s| s != cid.to_string()) => (
                false,
                format!(
                    "the response's cid {:?} differs from the signed record {cid}",
                    out.cid
                ),
            ),
            Ok(c) => (true, c.to_string()),
            Err(e) => (
                false,
                format!("the JSON value does not convert to DAG-CBOR: {e}"),
            ),
        },
        (Some(_), Err(XrpcCallError::RecordNotFound)) => (
            false,
            "the PDS reports RecordNotFound for a record its proof contains".to_owned(),
        ),
        (None, Err(XrpcCallError::RecordNotFound)) => (true, "RecordNotFound".to_owned()),
        (None, Ok(out)) => (
            false,
            format!(
                "the PDS returns a record (cid {}) that its proof says is absent",
                out.cid.unwrap_or_default()
            ),
        ),
        (_, Err(e)) => (false, e.to_string()),
    };
    r.check(checks::RECORD_JSON, ok, Some(detail));

    if deep {
        // Not a failure: nothing was checked. `ok` stays true so that consumers that require
        // every check to be ok do not see a failure next to a `proven` result.
        r.check(checks::DEEP, true, Some(DEEP_SKIPPED.to_owned()));
    }

    let all_ok = r
        .report
        .checks
        .iter()
        .filter(|c| c.name != checks::DEEP)
        .all(|c| c.ok);
    r.report.result = match (all_ok, proven) {
        (true, Some(_)) => VerifyOutcome::Proven,
        (true, None) => VerifyOutcome::Absent,
        (false, _) => VerifyOutcome::Failed,
    };
    Ok(r.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_mapping() {
        use lessdugin_atp::car::CarError;
        use lessdugin_atp::crypto::CryptoError;
        use lessdugin_atp::mst::MstError;
        assert_eq!(failing_stage(&ProofError::NoRoot), 0);
        assert_eq!(
            failing_stage(&ProofError::Car(CarError::TooLarge { len: 2, max: 1 })),
            0
        );
        assert_eq!(
            failing_stage(&ProofError::Commit(CommitError::Version(2))),
            1
        );
        assert_eq!(
            failing_stage(&ProofError::Commit(CommitError::FutureRev("x".into()))),
            2
        );
        assert_eq!(
            failing_stage(&ProofError::Commit(CommitError::Signature(
                CryptoError::BadSignature
            ))),
            3
        );
        assert_eq!(failing_stage(&ProofError::Mst(MstError::Ordering)), 4);
        let cid = lessdugin_atp::cid::for_raw(b"x");
        assert_eq!(failing_stage(&ProofError::MissingRecord(cid)), 5);
    }
}
