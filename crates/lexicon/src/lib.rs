// SPDX-License-Identifier: MIT OR Apache-2.0
//! lessdugin record types, the data-driven lexicon validator and record-key derivation.
//!
//! Normative source: `docs/spec/data-model.md` and `lexicons/online/7778777/lessdugin/`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod keys;
pub mod ns;
pub mod records;
pub mod schemas;
pub mod validate;
