// SPDX-License-Identifier: AGPL-3.0-or-later
//! Forum resolution and participant discovery (`sync.md` §4): references, opening a forum,
//! authorities, the roster (G, B, N, S), confirmation, the closure, newcomers, the sync order
//! and completeness.
//!
//! | Module | Contents |
//! |---|---|
//! | [`forum_ref`] | reference parsing (§4.1) and [`ForumKey`] (F, owner, slug, fk) |
//! | [`roster`] | authorities and ranks, G/B/N/S, members, the closure (pure) |
//! | [`state`] | discovery rows, the roster over stored records, C(F), completeness |
//! | [`forum`] | `open_forum` and `sync_forum` on [`crate::sync::Syncer`] |
//!
//! Newcomer *sources* (Constellation, Jetstream, indexer, relay; §4.4) are CORE+ (M5). Their
//! extension point is [`SyncOpts::newcomers`] (and [`SyncOpts::on_screen`] for step 2): DIDs
//! given there are confirmed and synced within the budget like any other candidate.

pub mod forum;
pub mod forum_ref;
pub mod roster;
pub mod state;

pub use forum::{
    DeclProof, DiscoveryError, ForumSync, NEWCOMER_BUDGET, OpenedForum, SyncOpts,
};
pub use forum_ref::{ForumKey, ForumRef, ForumRefError, parse_forum_ref};
pub use roster::{Roster, RosterRecord, authorities};
pub use state::{DiscRow, DiscState, ForumState, StatusCounts};
