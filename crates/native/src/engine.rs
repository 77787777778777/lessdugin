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
    Engine, EngineConfig, EngineError, IndexerMode as EngineIndexerMode, Sources as EngineSources,
    VerifyMode as EngineVerifyMode,
};
use lessdugin_core::identity::DnsMode as EngineDns;

use crate::config::{
    Config, IndexerSetting, NetworkProfile, Paths, Sources, VerifyMode, parity_now_ms,
};
use crate::host::NativeHost;

fn indexers(cfg: &Config) -> Vec<lessdugin_core::engine::IndexerConsent> {
    match &cfg.indexer {
        // Explicit URLs in the local configuration are the user's consent. Forum discovery and
        // "none" add no implicit consent; forum-scoped consent comes from the state store.
        IndexerSetting::Urls(urls) => urls
            .iter()
            .cloned()
            .map(|url| lessdugin_core::engine::IndexerConsent { url })
            .collect(),
        IndexerSetting::Mode(_) => Vec::new(),
    }
}

/// Maps the native configuration to the engine's (without parity mode; see [`open_engine`]).
pub fn engine_config(cfg: &Config) -> EngineConfig {
    // A file setting pins sources like a flag (O-9). A devnet is pinned to direct, whatever a
    // stored setting says (`EngineConfig::devnet`): only an explicit config.toml `sources` may
    // enable its explicitly configured local CORE+ services.
    let sources = cfg.sources.map(|s| match s {
        Sources::Direct => EngineSources::Direct,
        Sources::Assisted => EngineSources::Assisted,
    });
    let devnet = matches!(&cfg.network, NetworkProfile::Devnet(_));
    let sources_flag = if devnet {
        sources.or(Some(EngineSources::Direct))
    } else {
        sources
    };
    let common = EngineConfig {
        sources: sources_flag.unwrap_or(EngineSources::Assisted),
        sources_flag,
        verify: match cfg.verify {
            VerifyMode::Pds => EngineVerifyMode::Pds,
            VerifyMode::Proof => EngineVerifyMode::Proof,
        },
        doh: if devnet { Vec::new() } else { cfg.doh.clone() },
        // The native host has no system TXT resolver yet; DoH is used instead.
        dns: EngineDns::Doh,
        plc: cfg.plc.clone(),
        relays: cfg.relays.clone(),
        jetstream: cfg.jetstream.clone(),
        constellation: cfg.constellation.clone(),
        slingshot: cfg.slingshot.clone(),
        indexer_mode: match &cfg.indexer {
            IndexerSetting::Mode(crate::config::IndexerMode::None) => EngineIndexerMode::None,
            IndexerSetting::Mode(crate::config::IndexerMode::Forum) | IndexerSetting::Urls(_) => {
                EngineIndexerMode::Forum
            }
        },
        indexers: indexers(cfg),
        // An explicit URL list is the complete consent for this invocation. `forum`
        // includes the user's stored global and forum-scoped consent; `none` disables both.
        stored_indexer_consent: !matches!(cfg.indexer, IndexerSetting::Urls(_)),
        ..EngineConfig::default()
    };
    match &cfg.network {
        NetworkProfile::Devnet(d) => EngineConfig {
            network: lessdugin_core::engine::Network::Devnet {
                plc: d.plc.clone(),
                pds: d.pds.clone(),
            },
            plc: d.plc.clone(),
            // Only the devnet profile's explicitly configured socket is used.
            jetstream: d.jetstream.iter().cloned().collect(),
            ..common
        },
        NetworkProfile::Production => common,
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

    /// A devnet without a `sources` line is pinned to direct, so a stored `sources = assisted`
    /// never turns on CORE+ there (`EngineConfig::devnet`).
    #[tokio::test]
    async fn a_devnet_pins_direct_unless_the_file_states_sources() {
        let cfg = Config::devnet(
            url::Url::parse("http://127.0.0.1:2582").unwrap(),
            url::Url::parse("http://127.0.0.1:2583").unwrap(),
        );
        assert_eq!(cfg.sources, None);
        assert_eq!(engine_config(&cfg).sources_flag, Some(EngineSources::Direct));
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let engine = open_engine(&paths, &cfg).unwrap();
        let assisted = serde_json::json!("assisted");
        engine
            .set_setting("sources", Some(&assisted))
            .await
            .unwrap();
        assert_eq!(
            engine.settings().await.unwrap().sources,
            lessdugin_core::dto::Sources::Direct
        );
        let mut stated = cfg.clone();
        stated.sources = Some(Sources::Assisted);
        assert_eq!(
            engine_config(&stated).sources_flag,
            Some(EngineSources::Assisted)
        );
    }

    #[test]
    fn only_explicit_indexer_urls_become_configuration_consent() {
        let url = url::Url::parse("https://index.example.com").unwrap();
        let mut cfg = Config {
            indexer: crate::config::IndexerSetting::Urls(vec![url.clone()]),
            ..Config::default()
        };
        assert_eq!(engine_config(&cfg).indexers.len(), 1);
        assert_eq!(engine_config(&cfg).indexers[0].url, url);
        cfg.indexer = crate::config::IndexerSetting::Mode(crate::config::IndexerMode::Forum);
        assert!(engine_config(&cfg).indexers.is_empty());
        cfg.indexer = crate::config::IndexerSetting::Mode(crate::config::IndexerMode::None);
        assert!(engine_config(&cfg).indexers.is_empty());

        let mut devnet = Config::devnet(
            url::Url::parse("http://127.0.0.1:2582").unwrap(),
            url::Url::parse("http://127.0.0.1:2583").unwrap(),
        );
        devnet.indexer = crate::config::IndexerSetting::Urls(vec![url]);
        assert_eq!(engine_config(&devnet).indexers.len(), 1);
    }

    #[test]
    fn explicit_assisted_devnet_uses_only_its_configured_services() {
        let cfg = Config::from_toml_str(
            r#"sources = "assisted"
            relays = []
            jetstream = []
            constellation = false
            slingshot = false
            [network]
            mode = "devnet"
            plc = "http://localhost:2582"
            pds = "http://localhost:2583""#,
        )
        .unwrap();
        let c = engine_config(&cfg);
        assert_eq!(c.sources_flag, Some(EngineSources::Assisted));
        assert!(c.relays.is_empty() && c.jetstream.is_empty());
        assert_eq!(c.constellation, None);
        assert_eq!(c.slingshot, None);
        assert_eq!(
            engine_config(&Config::default()).sources,
            EngineSources::Assisted
        );
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
