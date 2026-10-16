// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal, wasm-safe AT Protocol primitives used by lessdugin.
//!
//! This crate performs no I/O and never reads clocks: callers pass time in. It never panics on
//! malformed input; every decoder has explicit caps (depth, element counts, sizes) that are checked
//! before allocating or recursing.
//!
//! | Module | Contents |
//! |---|---|
//! | [`syntax`] | [`syntax::Did`], [`syntax::Handle`], [`syntax::Nsid`], [`syntax::RecordKey`], [`syntax::AtUri`] (canonical, with the lessdugin DID policy), [`syntax::LooseAtUri`] (lenient), [`syntax::Datetime`] |
//! | [`tid`] | [`tid::Tid`], [`tid::TidGenerator`] (ignores far-future observed revs) |
//! | [`cid`] | CIDv1 helpers over [`Cid`] (sha-256, dag-cbor / raw) |
//! | [`dagcbor`] | strict canonical DAG-CBOR codec over [`Ipld`]; allocation-free validation and raw navigation |
//! | [`datamodel`] | atproto JSON ⇄ IPLD (`$link`, `$bytes`, blobs), blessed CIDs only, record CIDs, blob refs, nesting depth |
//! | [`crypto`] | did:key / Multikey public keys (compressed points), low-S ECDSA verification |
//! | [`did_doc`] | DID documents per `sync.md` §3.2 (`#atproto` key, `#atproto_pds` endpoint, claimed handle) under a [`did_doc::DocPolicy`] |
//! | [`car`] | CARv1 reader/writer with hard caps ([`car::CarLimits::PROOF`], [`car::CarLimits::BUNDLE`]) |
//! | [`mst`] | MST node codec, key heights, the `sync.md` §8.5 walk, deterministic builder |
//! | [`commit`] | v3 commit decode and verification (`sync.md` §8.4) |
//! | [`proof`] | `proofs/1` (`sync.md` §8): [`proof::verify_proof`], [`proof::verify_proof_in`], [`proof::ProofBundle`], error kinds |
//! | [`xrpc`] | request/response structs for the XRPC subset lessdugin uses |
//!
//! See `docs/spec/sync.md` §3.2 and §8 (`proofs/1`), `docs/spec/data-model.md` and
//! `docs/spec/limits.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod car;
pub mod cid;
pub mod commit;
pub mod crypto;
pub mod dagcbor;
pub mod datamodel;
pub mod did_doc;
pub mod error;
pub mod mst;
pub mod proof;
pub mod syntax;
pub mod tid;
pub mod xrpc;

pub use ::cid::Cid;
pub use error::Error;
pub use ipld_core;
pub use ipld_core::ipld::Ipld;
