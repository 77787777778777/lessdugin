// SPDX-License-Identifier: AGPL-3.0-or-later
//! `kvConformance(store)` (feature `kv-conformance`, test builds only): the shared `KvStore`
//! contract (`lessdugin_testkit::kv::kv_conformance`) and record-store scenario
//! (`lessdugin_testkit::store::record_store_conformance`) run against a JS `KvBackend` through
//! the same [`JsKv`] adapter the engine uses, so the IndexedDB store passes exactly the suites
//! `MemKv` and `SqliteKv` pass (`sync.md` §12.4). A violation panics; the panic hook reports it
//! on `console.error` and the call rejects (a wasm trap).

use wasm_bindgen::prelude::*;

use crate::host::{JsKv, JsKvBackend};

/// Runs both suites on `store`, which must start empty.
#[wasm_bindgen(js_name = kvConformance)]
pub async fn kv_conformance(store: JsKvBackend) {
    crate::bindings::install_panic_hook();
    let kv = JsKv::new(store);
    lessdugin_testkit::kv::kv_conformance(&kv).await;
    lessdugin_testkit::store::record_store_conformance(&kv).await;
}
