// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command context: paths, configuration, the native host and the engine (opened lazily),
//! the session store, and account selection.
//!
//! Devnet mode (`threat-model.md` INFRA-9, `sync.md` §3.7) exists only in builds with
//! [`DEVNET_AVAILABLE`]. When it is on, a warning is printed on every run, and the data and
//! cache live in the devnet's own directories ([`Paths::for_devnet`]) with its own keyring
//! service, so a devnet can never answer for, or leave accounts and issuer pins in, the
//! production stores.

use std::sync::OnceLock;

use lessdugin_atp::syntax::Did;
use lessdugin_core::dto::AccountView;
use lessdugin_core::engine::{Engine, EngineConfig, EngineError, ErrorCode};
use lessdugin_core::outbox::Session;
use lessdugin_native::auth::{SecretStore, restore_session};
use lessdugin_native::config::{Config, DevnetProfile, DnsMode, NetworkProfile, Paths};
use lessdugin_native::host::NativeHost;

use crate::args::GlobalArgs;
use crate::out::{Printer, tt};
use crate::state::CliState;

pub use lessdugin_native::engine::engine_config;

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

/// True in builds that have devnet mode: debug builds, and release builds with the `devnet`
/// feature. Other builds refuse `--dev` and devnet profiles.
pub const DEVNET_AVAILABLE: bool = cfg!(any(debug_assertions, feature = "devnet"));

fn devnet_unavailable() -> CliError {
    CliError::usage(
        "devnet mode (--dev, or a [network] mode = \"devnet\" profile) is not available in this \
         build; it is for development builds only",
    )
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
    /// The CLI's own state: the current forum and this terminal's listings.
    pub state: CliState,
    /// The keyring service of the session store (a devnet's own in devnet mode).
    keyring_service: Option<String>,
    /// The test-only parity-mode time (`LESSDUGIN_PARITY_NOW_MS`, devnet only;
    /// `engine-api.md` §2.1).
    parity_now_ms: Option<i64>,
    engine: OnceLock<Engine<NativeHost>>,
}

/// An engine acting as one logged-in account, with its stored session registered for its
/// PDS requests (the writing commands).
pub struct Writer {
    /// The engine, acting as the account.
    pub engine: Engine<NativeHost>,
    /// The account (re-checked against its DID document).
    pub account: AccountView,
    /// Its outbox session.
    pub session: Session,
}

impl Ctx {
    /// Resolves paths, loads `config.toml` and applies `--dev`/`--plc-url`/`--pds-url`.
    pub fn new(g: GlobalArgs) -> Result<Self, CliError> {
        let out = Printer::new(g.json, g.color, g.quiet);
        let mut paths = Paths::resolve(g.data_dir.as_deref())
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let (mut config, config_problem) = match Config::load(&paths.config_file) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        if !DEVNET_AVAILABLE && (g.dev || config.is_devnet()) {
            return Err(devnet_unavailable());
        }
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
        let mut keyring_service = None;
        if let NetworkProfile::Devnet(d) = &config.network {
            paths = paths.for_devnet(d);
            keyring_service = Some(d.keyring_service());
            out.warn(tt(&format!(
                "devnet mode: PLC {} and PDS {} (plain http and loopback allowed); devnet data \
                 is kept in {}",
                d.plc,
                d.pds,
                paths.data_dir.display()
            )));
        }
        // test-only parity mode (`engine-api.md` §2.1): devnet only
        let parity_now_ms = lessdugin_native::config::parity_now_ms(&config)
            .map_err(|e| CliError::usage(e.to_string()))?;
        if let Some(t) = parity_now_ms {
            out.warn(tt(&format!(
                "parity mode: views are computed at the fixed time {t} ms (tests only)"
            )));
        }
        let state = CliState::new(&paths);
        Ok(Self {
            g,
            out,
            paths,
            config,
            config_problem,
            state,
            keyring_service,
            parity_now_ms,
            engine: OnceLock::new(),
        })
    }

    /// The engine over the native host (opens the stores on first use).
    pub fn engine(&self) -> Result<&Engine<NativeHost>, CliError> {
        if let Some(e) = self.engine.get() {
            return Ok(e);
        }
        let host = NativeHost::open(&self.paths, &self.config).map_err(EngineError::from)?;
        host.set_offline(self.g.offline);
        let engine = Engine::new(
            host,
            EngineConfig {
                parity_now_ms: self.parity_now_ms,
                ..engine_config(&self.config)
            },
        );
        Ok(self.engine.get_or_init(|| engine))
    }

    /// The session store (the devnet's own keyring service in devnet mode).
    pub fn store(&self) -> SecretStore {
        let store = SecretStore::new(&self.paths);
        match &self.keyring_service {
            Some(s) => store.with_keyring_service(s.clone()),
            None => store,
        }
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

    /// The account, re-checked against its DID document, with its stored session registered
    /// for `UserPds` requests.
    pub async fn account_with_session(&self) -> Result<AccountView, CliError> {
        let account = self.account().await?;
        let engine = self.engine()?;
        Ok(restore_session(engine, &self.store(), &self.paths, &account).await?)
    }

    /// True with `--offline`.
    pub fn offline(&self) -> bool {
        self.g.offline
    }

    /// The engine for queries: acting as `--account` when given (the viewer-dependent fields
    /// are that account's), else the plain engine (the active account, or anonymous).
    pub async fn reader(&self) -> Result<Engine<NativeHost>, CliError> {
        let engine = self.engine()?;
        match &self.g.account {
            Some(_) => {
                let a = self.account().await?;
                Ok(engine.acting_as(&a.did).await?)
            }
            None => Ok(engine.clone()),
        }
    }

    /// The writing engine for new content: the account (`--account` or the active one) with
    /// its session restored.
    pub async fn writer(&self) -> Result<Writer, CliError> {
        let account = self.account_with_session().await?;
        self.writer_of(account).await
    }

    /// The writing engine of the logged-in account `did` (edits, deletes and retracts are
    /// written by the record's owner): `auth_required` if it is not logged in here.
    pub async fn writer_for(&self, did: &str) -> Result<Writer, CliError> {
        let engine = self.engine()?;
        let Some(account) = engine.account(did).await? else {
            return Err(CliError::Engine(
                EngineError::new(
                    ErrorCode::AuthRequired,
                    format!("{did} is not logged in on this device; only its owner can change it"),
                )
                .with_detail("did", did),
            ));
        };
        let account = restore_session(engine, &self.store(), &self.paths, &account).await?;
        self.writer_of(account).await
    }

    async fn writer_of(&self, account: AccountView) -> Result<Writer, CliError> {
        let engine = self.engine()?.acting_as(&account.did).await?;
        let session = Session::new(
            Did::parse(&account.did).map_err(|e| EngineError::internal(e.to_string()))?,
            url::Url::parse(&account.pds)
                .map_err(|e| EngineError::internal(format!("PDS {}: {e}", account.pds)))?,
        );
        Ok(Writer {
            engine,
            account,
            session,
        })
    }
}

fn not_logged_in_as(want: &str) -> CliError {
    CliError::Engine(
        EngineError::new(ErrorCode::AuthRequired, format!("not logged in as {want}"))
            .with_detail("account", want),
    )
}

/// A recorded account by DID, or by handle (`@` optional, case-insensitive). A handle must
/// still be the account's handle: the recorded one is re-checked against the identity
/// ([`Engine::refresh_account`]), and a handle that moved to another recorded account selects
/// that one. Offline, the recorded handle is used as is.
pub async fn find_account(
    engine: &Engine<NativeHost>,
    want: &str,
) -> Result<AccountView, CliError> {
    let w = want.trim().trim_start_matches('@').to_ascii_lowercase();
    let accounts = engine.accounts().await?;
    if accounts.is_empty() {
        return Err(not_logged_in_as(want));
    }
    if w.starts_with("did:") {
        return accounts
            .into_iter()
            .find(|a| a.did == w)
            .ok_or_else(|| not_logged_in_as(want));
    }
    let recorded = accounts.iter().find(|a| {
        a.handle
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(&w))
    });
    if let Some(a) = recorded {
        match engine.refresh_account(&a.did).await {
            Ok(fresh) if fresh.handle.as_deref() == Some(w.as_str()) => return Ok(fresh),
            Ok(_) => {} // the handle moved: look it up below
            Err(e) => {
                tracing::debug!(error = %e.message, "could not re-check the account's handle");
                return Ok(a.clone());
            }
        }
    }
    match engine.resolve_actor(&w).await {
        Ok(actor) if actor.handle.as_deref() == Some(w.as_str()) => {
            match accounts.iter().find(|a| a.did == actor.did) {
                Some(a) => Ok(engine.refresh_account(&a.did).await.unwrap_or(a.clone())),
                None => Err(not_logged_in_as(want)),
            }
        }
        _ => Err(not_logged_in_as(want)),
    }
}
