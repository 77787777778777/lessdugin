// SPDX-License-Identifier: AGPL-3.0-or-later
//! The wasm-bindgen exports: `WasmEngine` and `engineVersion`.

use js_sys::{Function, Promise};
use lessdugin_core::dto::Event;
use lessdugin_core::engine::{Engine, EngineError, SubscriptionId};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::host::{JsHost, JsHostImports};

/// Version of the engine compiled into this module.
#[wasm_bindgen(js_name = engineVersion)]
pub fn engine_version() -> String {
    super::engine_version().to_owned()
}

/// The NSID prefix this module reads and writes (`online.7778777.lessdugin.temp` until the
/// `final-ns` build). The main thread's AuthBridge has its own build-time copy and the worker
/// refuses to start when they differ.
#[wasm_bindgen(js_name = engineNamespace)]
pub fn engine_namespace() -> String {
    lessdugin_lexicon::ns::NS.to_owned()
}

/// An `EngineError` as the JSON text a rejected `call` carries.
fn error_json(e: &EngineError) -> JsValue {
    JsValue::from_str(&serde_json::to_string(e).unwrap_or_else(|_| {
        r#"{"code":"internal","message":"unserialisable error","retryable":false}"#.into()
    }))
}

#[cfg(feature = "debug-panic")]
pub(crate) fn install_panic_hook() {
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = console, js_name = error)]
        fn console_error(s: &str);
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            console_error(&format!("lessdugin-wasm: {info}"))
        }));
    });
}

/// The engine of the worker.
#[wasm_bindgen]
pub struct WasmEngine {
    engine: Engine<JsHost>,
}

#[wasm_bindgen]
impl WasmEngine {
    /// `new WasmEngine(configJson, host)`: the browser engine over the worker's host object.
    /// Throws an `EngineError` JSON string for an invalid configuration.
    #[wasm_bindgen(constructor)]
    pub fn new(config_json: &str, host: JsHostImports) -> Result<WasmEngine, JsValue> {
        #[cfg(feature = "debug-panic")]
        install_panic_hook();
        let config = crate::config::parse(config_json).map_err(|e| error_json(&e))?;
        Ok(Self {
            engine: Engine::new(JsHost::new(host), config),
        })
    }

    /// `call(op, argsJson)`: runs the operation `op` (an `EngineApi` method name) with its
    /// arguments as a JSON array; resolves with the result's dto/1 JSON, rejects with an
    /// `EngineError` JSON string.
    pub fn call(&self, op: String, args_json: String) -> Promise {
        let engine = self.engine.clone();
        future_to_promise(async move {
            crate::ops::call(&engine, &op, &args_json)
                .await
                .map(|s| JsValue::from_str(&s))
                .map_err(|e| error_json(&e))
        })
    }

    /// `subscribe(cb)`: `cb(eventJson)` receives every engine event as dto/1 JSON, in order and
    /// never re-entrantly. Returns the subscription id for `unsubscribe`.
    pub fn subscribe(&self, cb: Function) -> f64 {
        let id = self.engine.subscribe_boxed(Box::new(move |e: Event| {
            if let Ok(s) = serde_json::to_string(&e) {
                // a throwing listener must not stop the delivery to the others
                let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(&s));
            }
        }));
        id.0 as f64
    }

    /// Removes a subscription; `false` if it was not subscribed.
    pub fn unsubscribe(&self, id: f64) -> bool {
        self.engine.unsubscribe(SubscriptionId(id as u64))
    }
}
