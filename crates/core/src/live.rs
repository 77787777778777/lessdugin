// SPDX-License-Identifier: AGPL-3.0-or-later
//! Jetstream live updates (`sync.md` §6.2–§6.5).
//!
//! The wire parser and filter are pure. A Jetstream record is deliberately never returned by
//! this module: it is an unverified hint, so consumers can only point-fetch it from the author's
//! PDS. [`source::LiveSource`] owns no tasks; its caller drives it and the host supplies all I/O.

pub mod events;
pub mod source;
pub mod wire;

pub use events::{Action, Context, EventFilter, FilterOutput, Gap, Operation};
pub use source::{
    CURSOR_KEY, Cursor, DEDUP_CAPACITY, FilterMode, LiveConfig, LiveError, LiveSource,
    MAX_MESSAGE_BYTES, SourceStatus, Subscription,
};
pub use wire::{Event, EventKind, Wire};
