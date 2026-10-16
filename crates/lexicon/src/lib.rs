// SPDX-License-Identifier: MIT OR Apache-2.0
//! lessdugin record types, the data-driven lexicon validator and record-key derivation.
//!
//! Normative source: `docs/spec/data-model.md` and `lexicons/online/7778777/lessdugin/`.
//! The crate is wasm-safe: no I/O, no clocks, no threads; it never panics on input.
//!
//! | Module | Contents |
//! |---|---|
//! | [`ns`] | the build prefix `NS` (`.temp` unless the `final-ns` feature) and the prefix mapping (§1.3) |
//! | [`collection`] | the ten collections and every build-namespace NSID / `$type` constant |
//! | [`schemas`] | the embedded lexicon JSON and the compiled schema [`schemas::Registry`] (keyed by build NSID) |
//! | [`validate`] | the data-driven Lexicon validator over `serde_json::Value` (all violations, with JSON paths) |
//! | [`records`] | owned serde structs for every record and shared def; unknown fields preserved; [`records::Record`] |
//! | [`keys`] | `fk/1`, `sk/1`: `b32s`, `H`, `fk`, `h16`, `threadTid`, `stid`, `P`, `actionKey`, every rkey, the rkey grammars |
//! | [`validity`] | single-record validity, rules G1–G10 and the per-collection rules (§7.2–§7.3); entry-level helpers |
//! | [`values`] | known values and the unknown-value fallbacks (§13.3) |
//! | [`limits`] | normative validity limits and the §7.4 fetch caps |
//! | [`reserved`] | reserved names (§9) |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod collection;
pub mod keys;
pub mod limits;
pub mod ns;
pub mod records;
pub mod reserved;
pub mod schemas;
pub mod validate;
pub mod validity;
pub mod values;

pub use collection::{Collection, nsid};
pub use records::Record;
pub use validity::{RuleId, Validity, check};
