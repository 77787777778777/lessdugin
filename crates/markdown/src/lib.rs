// SPDX-License-Identifier: AGPL-3.0-or-later
//! LFM/1 (`lfm/1`): the lessdugin markdown profile.
//!
//! One event pipeline produces three outputs — sanitised HTML+MathML, terminal
//! [`term::StyledDoc`] lines, and the plain-text projection used for quote anchors.
//! Normative source: `docs/spec/lfm-1.md`.

#![forbid(unsafe_code)]

pub mod html;
pub mod lfm;
pub mod plain;
pub mod quote;
pub mod term;
pub mod url;
