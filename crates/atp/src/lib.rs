// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal, wasm-safe AT Protocol primitives used by lessdugin.
//!
//! This crate performs no I/O and never reads clocks: callers pass time in.
//! See `docs/spec/sync.md` (verification, `proofs/1`) and `docs/spec/data-model.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod car;
pub mod cid;
pub mod commit;
pub mod crypto;
pub mod datamodel;
pub mod did_doc;
pub mod mst;
pub mod syntax;
pub mod tid;
pub mod xrpc;
