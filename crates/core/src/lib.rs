// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lessdugin engine, shared by the CLI, TUI, SPA (via wasm) and the optional indexer.
//!
//! The core performs all I/O through the [`host::Host`] trait and never spawns tasks,
//! reads clocks or sleeps by itself. Normative sources: `docs/spec/sync.md`,
//! `docs/spec/social.md`, `docs/spec/data-model.md`.

#![forbid(unsafe_code)]

pub mod discovery;
pub mod dto;
pub mod engine;
pub mod host;
pub mod identity;
pub mod jetstream;
pub mod limits;
pub mod media;
pub mod model;
pub mod notify;
pub mod outbox;
pub mod search;
pub mod social;
pub mod store;
pub mod sync;
pub mod verify;
pub mod xrpc;
