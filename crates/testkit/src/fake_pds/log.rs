// SPDX-License-Identifier: AGPL-3.0-or-later
//! The request log and the repo event log (a firehose-like record of commits, identity and
//! account changes, for assertions and for a mini-Jetstream).

use std::collections::BTreeMap;

use lessdugin_atp::Cid;
use lessdugin_atp::syntax::{Did, Tid};

use super::repo::RepoOp;

/// The service a request reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// A PDS (by base URL); `userpds://` requests are logged with their account's PDS.
    Pds,
    /// The PLC directory.
    Plc,
    /// A relay (`listReposByCollection`).
    Relay,
    /// A DNS-over-HTTPS endpoint (`_atproto` TXT lookups).
    Doh,
    /// Nothing on the fake network.
    Unknown,
}

/// One request the fake network answered.
#[derive(Clone, Debug)]
pub struct LoggedRequest {
    /// Sequence number (from 1).
    pub seq: u64,
    /// Fake time at arrival (ms).
    pub at_ms: i64,
    /// Caller label.
    pub caller: String,
    /// HTTP method.
    pub method: http::Method,
    /// Full URL (`userpds://<did><path>` for authenticated host requests).
    pub url: String,
    /// The service.
    pub service: ServiceKind,
    /// The service base URL.
    pub host: String,
    /// XRPC NSID (`plc` for DID documents, `doh` for DNS).
    pub nsid: Option<String>,
    /// Query parameters.
    pub params: BTreeMap<String, String>,
    /// Subject repo DID, if any.
    pub did: Option<String>,
    /// The account the request was authenticated as.
    pub auth: Option<String>,
    /// Response status (0 when the connection was dropped).
    pub status: u16,
    /// XRPC error name of the response, if any.
    pub error: Option<String>,
    /// Descriptions of the faults applied.
    pub faults: Vec<String>,
    /// True if an equivocation fork answered.
    pub fork: bool,
}

/// A repo event.
#[derive(Clone, Debug)]
pub struct RepoEvent {
    /// Sequence number (from 1, across all repos).
    pub seq: u64,
    /// Fake time (µs).
    pub time_us: i64,
    /// The repo.
    pub did: Did,
    /// What happened.
    pub kind: RepoEventKind,
}

/// What a [`RepoEvent`] reports.
#[derive(Clone, Debug)]
pub enum RepoEventKind {
    /// A commit.
    Commit {
        /// Revision.
        rev: Tid,
        /// Commit CID.
        cid: Cid,
        /// The operations (one per path, in path order).
        ops: Vec<RepoOp>,
    },
    /// An identity change (handle, key, PDS or document).
    Identity {
        /// The current handle.
        handle: Option<String>,
    },
    /// An account status change.
    Account {
        /// Active.
        active: bool,
        /// Status when inactive.
        status: Option<String>,
    },
}
