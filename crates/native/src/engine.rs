// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opening the engine over the native host (the CLI and the TUI share this, so both read and
//! write the same data directory the same way).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use lessdugin_native::config::{Config, Paths};
//! let paths = Paths::resolve(None)?;
//! let config = Config::load(&paths.config_file)?;
//! // a devnet profile keeps its data in its own directories (`Paths::for_devnet`)
//! let engine = lessdugin_native::engine::open_engine(&paths, &config)?;
//! # let _ = engine;
//! # Ok(())
//! # }
//! ```

use lessdugin_core::engine::{
    Engine, EngineConfig, EngineError, Sources as EngineSources, VerifyMode as EngineVerifyMode,
};
use lessdugin_core::identity::DnsMode as EngineDns;

use crate::config::{Config, NetworkProfile, Paths, Sources, VerifyMode, parity_now_ms};
use crate::host::NativeHost;

/// Maps the native configuration to the engine's (without parity mode; see [`open_engine`]).
pub fn engine_config(cfg: &Config) -> EngineConfig {
    match &cfg.network {
        NetworkProfile::Devnet(d) => EngineConfig {
            network: lessdugin_core::engine::Network::Devnet {
                plc: d.plc.clone(),
                pds: d.pds.clone(),
            },
            sources: match cfg.sources.unwrap_or(Sources::Direct) {
                Sources::Direct => EngineSources::Direct,
                Sources::Assisted => EngineSources::Assisted,
            },
            sources_flag: cfg.sources.map(|s| match s {
                Sources::Direct => EngineSources::Direct,
                Sources::Assisted => EngineSources::Assisted,
            }),
            verify: match cfg.verify {
                VerifyMode::Pds => EngineVerifyMode::Pds,
                VerifyMode::Proof => EngineVerifyMode::Proof,
            },
            doh: Vec::new(),
            plc: d.plc.clone(),
            // Production defaults are never a devnet's CORE+ configuration. Config::devnet
            // starts empty; only explicit configured endpoints are copied here.
            relays: cfg.relays.clone(),
            jetstream: d.jetstream.iter().cloned().collect(),
            constellation: cfg.constellation.clone(),
            slingshot: cfg.slingshot.clone(),
            indexers: Vec::new(),
            platform: lessdugin_core::engine::Platform::Native,
            ..EngineConfig::default()
        },
        NetworkProfile::Production => EngineConfig {
            // a `sources` the file states pins it like a flag (O-9): a CLI flag, then
            // config.toml, then the stored setting, then the build default (`sync.md` §2.3)
            sources_flag: cfg.sources.map(|s| match s {
                Sources::Direct => EngineSources::Direct,
                Sources::Assisted => EngineSources::Assisted,
            }),
            verify: match cfg.verify {
                VerifyMode::Pds => EngineVerifyMode::Pds,
                VerifyMode::Proof => EngineVerifyMode::Proof,
            },
            doh: cfg.doh.clone(),
            // The native host has no system TXT resolver yet; DoH is used instead.
            dns: EngineDns::Doh,
            plc: cfg.plc.clone(),
            relays: cfg.relays.clone(),
            jetstream: cfg.jetstream.clone(),
            constellation: cfg.constellation.clone(),
            slingshot: cfg.slingshot.clone(),
            ..EngineConfig::default()
        },
    }
}

/// Opens the stores under `paths` (which must already be the devnet's own paths in devnet
/// mode, [`Paths::for_devnet`]) and returns the engine for `config`, with the test-only parity
/// time of `LESSDUGIN_PARITY_NOW_MS` (`engine-api.md` §2.1; refused outside devnet mode).
pub fn open_engine(paths: &Paths, config: &Config) -> Result<Engine<NativeHost>, EngineError> {
    let parity = parity_now_ms(config).map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let host = NativeHost::open(paths, config).map_err(EngineError::from)?;
    Ok(Engine::new(
        host,
        EngineConfig {
            parity_now_ms: parity,
            ..engine_config(config)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_devnet_never_inherits_production_core_plus_endpoints() {
        let mut cfg = Config::devnet(
            url::Url::parse("http://127.0.0.1:2582").unwrap(),
            url::Url::parse("http://127.0.0.1:2583").unwrap(),
        );
        cfg.sources = Some(Sources::Assisted);
        let c = engine_config(&cfg);
        assert!(c.relays.is_empty());
        assert!(c.jetstream.is_empty());
        assert!(c.constellation.is_none());
        assert!(c.slingshot.is_none());
        assert!(c.doh.is_empty());
        assert_eq!(c.sources, EngineSources::Assisted);
    }

    /// O-9: a `sources` in config.toml wins over the stored setting; without one the stored
    /// setting decides.
    #[tokio::test]
    async fn config_sources_pin_the_stored_setting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let pinned = Config::from_toml_str("sources = \"direct\"").unwrap();
        assert_eq!(
            engine_config(&pinned).sources_flag,
            Some(EngineSources::Direct)
        );
        let engine = open_engine(&paths, &pinned).unwrap();
        let assisted = serde_json::json!("assisted");
        engine
            .set_setting("sources", Some(&assisted))
            .await
            .unwrap();
        assert_eq!(
            engine.settings().await.unwrap().sources,
            lessdugin_core::dto::Sources::Direct
        );
        drop(engine);
        // the same stored setting, with a file that does not state `sources`
        let free = Config::default();
        assert_eq!(engine_config(&free).sources_flag, None);
        let engine = open_engine(&paths, &free).unwrap();
        assert_eq!(
            engine.settings().await.unwrap().sources,
            lessdugin_core::dto::Sources::Assisted
        );
        let direct = serde_json::json!("direct");
        engine.set_setting("sources", Some(&direct)).await.unwrap();
        assert_eq!(
            engine.settings().await.unwrap().sources,
            lessdugin_core::dto::Sources::Direct
        );
    }
}
