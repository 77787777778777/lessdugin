// SPDX-License-Identifier: AGPL-3.0-or-later
//! WebAssembly bindings for the lessdugin engine. The SPA runs this module in a Web Worker.
//!
//! ## `JsHost` (M2, not written yet)
//!
//! The worker's `lessdugin_core::host::Host` implementation. Its contract is in
//! `crates/core/src/host.rs` and `sync.md` §12; one part that has no native counterpart:
//!
//! - **`Host::lock(name, wait)`** (`sync.md` §7.1, §10.6) MUST take the Web Lock
//!   `lessdugin-<name>` with `:` replaced by `-` (`outbox:<did>` → `lessdugin-outbox-<did>`)
//!   through `navigator.locks.request(lockName, { mode: "exclusive", ifAvailable: !wait },
//!   callback)`. The callback resolves the Rust future with a guard (`type Lock`) and returns a
//!   promise that the guard resolves when it is dropped, which releases the lock; with
//!   `ifAvailable` a `null` lock answers `Ok(None)`. Web Locks exclude other tabs and other
//!   tasks of this worker alike, and are released when the tab closes. The outbox takes
//!   `outbox:<did>` around every send, so a `JsHost` without it breaks "one write in flight per
//!   account" across tabs and with `lessdugin web`'s bridge.

#![forbid(unsafe_code)]

/// Version of the engine compiled into this module.
pub fn engine_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    use wasm_bindgen::prelude::*;

    /// Version of the engine compiled into this module.
    #[wasm_bindgen(js_name = engineVersion)]
    pub fn engine_version() -> String {
        super::engine_version().to_owned()
    }
}
