// SPDX-License-Identifier: AGPL-3.0-or-later
//! WebAssembly bindings for the lessdugin engine. The SPA runs this module in a Web Worker
//! (`packages/engine/src/worker.ts`, `engine-api.md` §5).
//!
//! | Module | Contents |
//! |---|---|
//! | [`ops`] | the operation dispatcher: `call(op, args)` over [`Engine`](lessdugin_core::engine::Engine), dto/1 JSON in and out (target-independent, tested natively) |
//! | [`config`] | the worker's configuration (JSON) → [`EngineConfig`](lessdugin_core::engine::EngineConfig) |
//! | `host` (wasm32) | `JsHost`: the core `Host` over the worker's JS host object (fetch, IndexedDB, Web Locks, timers, `crypto`) |
//! | `bindings` (wasm32) | `WasmEngine`, `engineVersion` (wasm-bindgen exports) |
//! | `conformance` (wasm32, feature `kv-conformance`) | `kvConformance(store)`: the testkit `KvStore` suites against a JS store |
//!
//! ## The boundary
//!
//! Every value crosses as **dto/1 JSON text**, exactly as `dto.rs` serialises it (the CLI's
//! `--json` bytes): `WasmEngine.call(op, argsJson)` takes the operation's camelCase name and a
//! JSON array of its arguments, and resolves with the result's JSON (`"null"` for none) or
//! rejects with an `EngineError` JSON string. Events reach `WasmEngine.subscribe(cb)` as JSON
//! strings. The worker parses them (`JSON.parse` never sets prototypes, threat-model SPA-11)
//! and can hand the untouched text to parity checks. No tsify or serde-wasm-bindgen: one JSON
//! codec, already linked for records, keeps the module small (ADR 0002).
//!
//! ## `JsHost`
//!
//! The worker's `lessdugin_core::host::Host` (`sync.md` §12). It calls the JS host object the
//! worker passes to `new WasmEngine(configJson, host)`; the JS side owns the platform rules:
//!
//! - **HTTP** (`host.http(req)`): `fetch` with `cache: "no-store"` (`sync.md` §12.1), the byte
//!   cap counted while streaming after decompression, the total timeout, the browser URL policy
//!   (threat-model §5.6.2), and `Target::UserPds` requests forwarded to the main thread's
//!   AuthBridge (the worker holds no credentials). Failures reject with
//!   `{ kind, message }`, `kind` naming a [`HostError`](lessdugin_core::host::HostError) variant.
//! - **Blobs** (`host.fetchBlob`): streamed and hashed in JS, kept as a `Blob` keyed by CID;
//!   no blob byte enters wasm memory (INFRA-6).
//! - **WebSocket**: unsupported in M2 (Jetstream arrives with M5; `live_tail` polls).
//! - **Stores** (`host.cache`, `host.state`): IndexedDB object stores with the raw tuple keys
//!   of `store::key` as binary keys, which IndexedDB orders as unsigned bytes like `MemKv`.
//! - **Locks** (`host.lock(name, wait)`): the Web Lock `lessdugin-<name>` with the first `:`
//!   replaced by `-` (`outbox:<did>` → `lessdugin-outbox-<did>`), exclusive, `ifAvailable`
//!   unless `wait`; the returned handle is released when the Rust guard drops.
//! - **Timers, clock, randomness**: `setTimeout`, `Date.now()`, `crypto.getRandomValues`.
//!
//! ## Parity mode
//!
//! `EngineConfig::parity_now_ms` (`engine-api.md` §2.1) is test-only: only an e2e build of the
//! SPA (a build-time flag, like the one that gates the mock engine) passes `parityNowMs` in the
//! worker configuration; release builds do not contain that code path (the worker drops the
//! field unless the flag is set).
//!
//! ## Size hygiene (ADR 0002 §6)
//!
//! ammonia (+266 KB gz) never enters the wasm32 build: [`ammonia_must_stay_out_of_wasm32`]
//! fails the build if `lessdugin-markdown`'s `ammonia` feature is on, and `cargo xtask wasm`
//! also checks the dependency tree. `url`'s IDNA back end is the ASCII-only `idna_adapter`
//! 1.0.x (see `Cargo.toml`).

#![forbid(unsafe_code)]

pub mod config;
pub mod ops;

#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(all(target_arch = "wasm32", feature = "kv-conformance"))]
mod conformance;
#[cfg(target_arch = "wasm32")]
mod host;

/// Version of the engine compiled into this module.
pub fn engine_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// ADR 0002 decision 3: ammonia costs +266 KB gz and must stay out of the wasm32 build. With
/// `lessdugin-markdown/ammonia` enabled, `allowlist::ammonia_clean` exists and the use below is
/// ambiguous between the two glob imports, which fails the wasm32 build here (error E0659,
/// "`ammonia_clean` is ambiguous"): the compile-time equivalent of a `compile_error!` on a
/// dependency's feature, which Cargo does not expose to dependents.
#[cfg(target_arch = "wasm32")]
pub mod ammonia_must_stay_out_of_wasm32 {
    mod absent {
        /// Present only so the check below resolves when ammonia is off.
        pub fn ammonia_clean() {}
    }
    #[allow(unused_imports)]
    use absent::*;
    #[allow(unused_imports)]
    use lessdugin_markdown::allowlist::*;

    /// Resolves to `absent::ammonia_clean` unless lessdugin-markdown was built with ammonia.
    pub const CHECK: fn() = ammonia_clean;
}
