// SPDX-License-Identifier: AGPL-3.0-or-later
//! Assisted-mode indexer client. JSON is hint-only; only independently verified `proofs/1`
//! bundles can become content. All I/O goes through the ordinary scheduled sync session.

pub(crate) mod audit;
pub(crate) mod changes;
pub(crate) mod client;
pub mod contract;
pub(crate) mod proofs;

pub use audit::{AuditCheck, AuditCounts, AuditFinding, AuditKind, AuditReport};
pub use changes::{ChangePollReport, LiveProofBatch};
pub use client::{Client, Consent, EndpointPolicy, endpoint_allowed};
pub use proofs::{ProofItemStatus, ProofReport, ProofStatus};
