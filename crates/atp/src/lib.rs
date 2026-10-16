// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal, wasm-safe AT Protocol primitives used by lessdugin.
//!
//! This crate performs no I/O and never reads clocks: callers pass time in. It never panics on
//! malformed input; every decoder has explicit caps (depth, element counts, sizes) that are checked
//! before allocating or recursing.
//!
//! | Module | Contents |
//! |---|---|
//! | [`syntax`] | [`syntax::Did`], [`syntax::Handle`], [`syntax::Nsid`], [`syntax::RecordKey`], [`syntax::AtUri`] (strict), [`syntax::LooseAtUri`] (lenient), [`syntax::Datetime`] |
//! | [`tid`] | [`tid::Tid`], [`tid::TidGenerator`] |
//! | [`cid`] | CIDv1 helpers over [`Cid`] (sha-256, dag-cbor / raw) |
//! | [`dagcbor`] | strict canonical DAG-CBOR codec over [`Ipld`] |
//! | [`datamodel`] | atproto JSON ⇄ IPLD (`$link`, `$bytes`, blobs), record CIDs, blob refs |
//! | [`crypto`] | did:key / Multikey public keys, low-S ECDSA verification |
//! | [`did_doc`] | DID document parsing (`#atproto` key, `#atproto_pds`, handle) |
//! | [`car`] | CARv1 reader/writer with hard caps |
//! | [`mst`] | MST node codec, key heights, proof walk, deterministic builder |
//! | [`commit`] | v3 commit decode and verification |
//! | [`proof`] | `proofs/1`: [`proof::verify_record_proof`] |
//! | [`xrpc`] | request/response structs for the XRPC subset lessdugin uses |
//!
//! See `docs/design/synthesis-v1.md` §3.6 (`proofs/1`) and `docs/spec/data-model.md`.

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
