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
        NetworkProfile::Devnet(d) => EngineConfig::devnet(d.plc.clone(), d.pds.clone()),
        NetworkProfile::Production => EngineConfig {
            sources: match cfg.sources {
                Sources::Direct => EngineSources::Direct,
                Sources::Assisted => EngineSources::Assisted,
            },
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
