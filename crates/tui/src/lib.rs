// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lessdugin terminal UI (`docs/design/synthesis-v1.md` §6.11): an Elm-style
//! `State`/`Action`/`Effect` loop over ratatui.
//!
//! | Module | Contents |
//! |---|---|
//! | [`app`] | [`State`] and its screen stack (Home, Forum, Thread), [`Action`], [`Effect`], `update` |
//! | [`view`] | the pure view: `draw(&State, &mut Frame)` |
//! | [`keys`] | the key map and the help overlay's table |
//! | [`thread`] | the Thread screen's layout and navigation |
//! | [`body`] | bodies: the LFM/1 terminal projection laid out at a width |
//! | [`text`] | `TermText` helpers: every string drawn goes through `term_sanitize` |
//! | [`editor`] | `$EDITOR` templates, files and the editor chain |
//! | [`exec`] | engine effects as futures returning actions |
//! | [`runtime`] | the crossterm terminal loop ([`run`]) |
//! | [`headless`] | the same loop over a `TestBackend` ([`Headless`], [`capture_thread`]) |
//!
//! The CLI starts it (`lessdugin tui [forum]`, or bare `lessdugin` on a terminal with an
//! account) with an engine it opened (`crates/cli/src/ctx.rs`), so configuration, devnet and
//! parity mode (`engine-api.md` §2.1) are the CLI's.

#![forbid(unsafe_code)]

pub mod app;
pub mod body;
pub mod command;
pub mod editor;
pub mod exec;
pub mod headless;
pub mod image;
pub mod keys;
pub mod runtime;
pub mod text;
pub mod thread;
pub mod view;

pub use app::{Action, ActivityData, Effect, ForumScreen, ForumTab, State};
pub use editor::EditorConfig;
pub use headless::{CaptureOpts, Headless, HeadlessOpts, ThreadCapture, capture_thread};
pub use runtime::{Options, Report, run};
