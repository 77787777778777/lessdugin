// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command context: paths, configuration, the native host and the engine (opened lazily),
//! the session store, and account selection.

use std::sync::OnceLock;

use lessdugin_core::dto::AccountView;
use lessdugin_core::engine::{
    Engine, EngineConfig, EngineError, ErrorCode, Sources as EngineSources,
    VerifyMode as EngineVerifyMode,
};
use lessdugin_core::identity::DnsMode as EngineDns;
use lessdugin_native::auth::{SecretStore, restore_session};
use lessdugin_native::config::{
    Config, DevnetProfile, DnsMode, NetworkProfile, Paths, Sources, VerifyMode,
};
use lessdugin_native::host::NativeHost;

use crate::args::GlobalArgs;
use crate::out::{Printer, tt};

/// A command failure: an engine error (exit code by its code), a usage error (exit 2), or an
/// error already reported (exit code only).
#[derive(Debug)]
pub enum CliError {
    /// An engine error.
    Engine(EngineError),
    /// Wrong usage.
    Usage(String),
    /// Already printed; exit with this code.
    Reported(i32),
}

impl From<EngineError> for CliError {
    fn from(e: EngineError) -> Self {
        Self::Engine(e)
    }
}

impl CliError {
    /// A usage error.
    pub fn usage(msg: impl Into<String>) -> Self {
        Self::Usage(msg.into())
    }
}

/// The context of one invocation.
pub struct Ctx {
    /// Global options.
    pub g: GlobalArgs,
    /// The printer.
    pub out: Printer,
    /// Paths.
    pub paths: Paths,
    /// The effective configuration (flags applied).
    pub config: Config,
    /// Why `config.toml` could not be used, if it could not (defaults are used instead).
    pub config_problem: Option<String>,
    engine: OnceLock<Engine<NativeHost>>,
}

/// Maps the native configuration to the engine's.
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

impl Ctx {
    /// Resolves paths, loads `config.toml` and applies `--dev`/`--plc-url`/`--pds-url`.
    pub fn new(g: GlobalArgs) -> Result<Self, CliError> {
        let out = Printer::new(g.json, g.color, g.quiet);
        let paths = Paths::resolve(g.data_dir.as_deref())
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let (mut config, config_problem) = match Config::load(&paths.config_file) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        if (g.plc_url.is_some() || g.pds_url.is_some()) && !g.dev {
            return Err(CliError::usage(
                "--plc-url and --pds-url select a devnet and need --dev",
            ));
        }
        if g.dev {
            let current = match &config.network {
                NetworkProfile::Devnet(d) => Some(d.clone()),
                NetworkProfile::Production => None,
            };
            let plc = g
                .plc_url
                .clone()
                .or(current.as_ref().map(|d| d.plc.clone()));
            let pds = g
                .pds_url
                .clone()
                .or(current.as_ref().map(|d| d.pds.clone()));
            let (Some(plc), Some(pds)) = (plc, pds) else {
                return Err(CliError::usage(
                    "--dev needs --plc-url and --pds-url (or a devnet [network] profile in \
                     config.toml)",
                ));
            };
            config.plc = plc.clone();
            config.network = NetworkProfile::Devnet(DevnetProfile {
                plc,
                pds,
                jetstream: current.and_then(|d| d.jetstream),
            });
            config
                .validate()
                .map_err(|e| CliError::usage(e.to_string()))?;
        }
        if config.dns == DnsMode::System && !config.is_devnet() {
            out.warn(tt(
                "dns = \"system\" is not available in this build; using DNS over HTTPS",
            ));
        }
        if let Some(p) = &config_problem {
            out.warn(tt(&format!("ignoring the configuration: {p}")));
        }
        Ok(Self {
            g,
            out,
            paths,
            config,
            config_problem,
            engine: OnceLock::new(),
        })
    }

    /// The engine over the native host (opens the stores on first use).
    pub fn engine(&self) -> Result<&Engine<NativeHost>, CliError> {
        if let Some(e) = self.engine.get() {
            return Ok(e);
        }
        let host = NativeHost::open(&self.paths, &self.config).map_err(EngineError::from)?;
        let engine = Engine::new(host, engine_config(&self.config));
        Ok(self.engine.get_or_init(|| engine))
    }

    /// The session store.
    pub fn store(&self) -> SecretStore {
        SecretStore::new(&self.paths)
    }

    /// The account to act as: `--account` (a recorded handle or DID) or the active one.
    pub async fn account(&self) -> Result<AccountView, CliError> {
        let engine = self.engine()?;
        match &self.g.account {
            Some(want) => find_account(engine, want).await,
            None => engine.active_account().await?.ok_or_else(|| {
                CliError::Engine(EngineError::new(ErrorCode::AuthRequired, "not logged in"))
            }),
        }
    }

    /// The account, with its stored session registered for `UserPds` requests.
    pub async fn account_with_session(&self) -> Result<AccountView, CliError> {
        let account = self.account().await?;
        let engine = self.engine()?;
        restore_session(engine.host(), &self.store(), &self.paths, &account).await?;
        Ok(account)
    }
}

/// A recorded account by handle (`@` optional, case-insensitive) or DID.
pub async fn find_account(
    engine: &Engine<NativeHost>,
    want: &str,
) -> Result<AccountView, CliError> {
    let w = want.trim().trim_start_matches('@').to_ascii_lowercase();
    let accounts = engine.accounts().await?;
    accounts
        .into_iter()
        .find(|a| {
            a.did == w
                || a.handle
                    .as_deref()
                    .is_some_and(|h| h.to_ascii_lowercase() == w)
        })
        .ok_or_else(|| {
            CliError::Engine(
                EngineError::new(ErrorCode::AuthRequired, format!("not logged in as {want}"))
                    .with_detail("account", want),
            )
        })
}
