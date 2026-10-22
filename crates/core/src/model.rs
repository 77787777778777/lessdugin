// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `ForumModel` (`sync.md` §13): per-forum in-memory skeletons built from one record-store
//! scan, the fold over them, body rendering with the `(cid, RENDERER_VERSION)` LRU, and the DTO
//! views.
//!
//! | Module | Contents |
//! |---|---|
//! | [`build`] | [`ForumModel::build`]: rows → fold input → fold |
//! | [`render`] | `md` bodies → [`crate::dto::RenderedBody`]; [`RenderCache`] |
//! | [`views`] | [`Views`]: post summaries and views, threads, history, members, forum header |
//!
//! The engine keeps one model per open forum and rebuilds it when a `Changed` event names the
//! forum (own writes, sync passes). Bodies are not kept in the model: views re-read the record
//! values they render.

pub mod build;
pub mod render;
pub mod views;

pub use build::{ForumModel, ModelError, RowMeta};
pub use render::RenderCache;
pub use views::Views;
