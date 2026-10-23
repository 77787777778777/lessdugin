// SPDX-License-Identifier: AGPL-3.0-or-later
//! The deterministic social fold (`docs/spec/social.md`): moderation (`mod/1`), karma
//! (`karma/1`), ranking (`magic/1`), reacts (`default/1`), threads and visibility.
//!
//! | Module | Contents |
//! |---|---|
//! | [`input`] | `FoldInput` (§1.1) and its skeleton form [`input::ForumInput`] |
//! | [`policy`] | the effective policy and `PARAMS` (§1.2, §1.6) |
//! | [`time`] | `µs`, `tidTime`, `t_rank` (§2) |
//! | [`threads`] | thread placement (§11), a pure function |
//! | [`full`] | the fold, every layer of §1.4 ([`full::fold_view`]; the engine's forum model runs [`full::fold_cx`]) |
//! | [`rank`] | `hot`, post and comment sorts (§12) |
//! | [`digest`] | `inputDigest` (§1.2) |
//!
//! Every function here is pure: no I/O, no clock (`now` is an input), ordered collections only
//! (§1.3).

pub mod digest;
pub mod full;
pub mod input;
pub mod policy;
pub mod rank;
pub mod threads;
pub mod time;

pub use input::{FoldInput, ForumInput, Skel, SkelData};
pub use policy::EffectivePolicy;

/// Why a fold could not run: a malformed input (§1.1, §20.1 item 25) or a declaration that
/// cannot be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("fold input: {0}")]
pub struct FoldError(pub String);
