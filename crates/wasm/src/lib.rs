// SPDX-License-Identifier: AGPL-3.0-or-later
//! WebAssembly bindings for the lessdugin engine. The SPA runs this module in a Web Worker.

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
