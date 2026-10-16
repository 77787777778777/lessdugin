// SPDX-License-Identifier: AGPL-3.0-or-later
//! atproto identifiers as LFM/1 uses them (`docs/spec/lfm-1.md` §5.2), on top of the strict
//! syntax types of `lessdugin-atp`.

pub use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::syntax::{Handle, Nsid, RecordKey};

/// A DID accepted in markdown: `^did:plc:[a-z2-7]{24}$` or
/// `^did:web:<lowercase hostname, no port, no path>$` (the lessdugin DID policy).
pub(crate) fn lfm_did(s: &str) -> Option<Did> {
    let did = Did::parse(s).ok()?;
    did.check_lessdugin_policy().ok()?;
    Some(did)
}

/// `at://did/nsid/rkey` from validated parts; `None` unless the NSID and record key are valid
/// atproto syntax and the NSID authority is lowercase (the canonical reference form).
pub(crate) fn record_uri(did: &Did, nsid: &str, rkey: &str) -> Option<AtUri> {
    let nsid = Nsid::parse(nsid).ok()?;
    let rkey = RecordKey::parse(rkey).ok()?;
    AtUri::new(did.clone(), nsid, rkey).ok()
}

/// A valid atproto handle, lowercased.
pub(crate) fn handle(s: &str) -> Option<String> {
    Handle::parse(s).ok().map(|h| h.as_str().to_owned())
}

/// The collection NSID of a record URI.
pub(crate) fn collection(uri: &AtUri) -> &str {
    uri.collection().as_str()
}
