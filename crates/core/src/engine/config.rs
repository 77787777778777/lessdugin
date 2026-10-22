// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`EngineConfig`] (`engine-api.md` §2, settings in `sync.md` §2.3).

use crate::identity::{DEFAULT_DOH, DEFAULT_PLC, DEFAULT_SLINGSHOT, DnsMode, IdentityConfig};

/// Which network the engine talks to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Network {
    /// The public atproto network.
    #[default]
    Production,
    /// A local devnet (`sync.md` §3.7, ADR 0006): handles resolve through `pds`'s
    /// `resolveHandle`, `.test` handles are valid, and `http`/IP-literal endpoints are allowed.
    /// Never enabled implicitly.
    Devnet {
        /// The devnet PLC directory.
        plc: url::Url,
        /// The devnet PDS.
        pds: url::Url,
    },
}

/// `sources` (`sync.md` §2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sources {
    /// CORE only (PDSes, PLC/did:web, DoH).
    Direct,
    /// CORE+ (relay, Jetstream, Constellation, Slingshot) and enabled indexers.
    #[default]
    Assisted,
}

/// `verify` (`sync.md` §2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VerifyMode {
    /// Admit content at level `pds`.
    #[default]
    Pds,
    /// Verify every displayed item in the background (Reserved, P2).
    Proof,
}

/// The platform the engine runs on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Platform {
    /// CLI, TUI, indexer.
    #[default]
    Native,
    /// The SPA (Web Worker): no system DNS, CORS rules.
    Browser,
}

/// A consented indexer endpoint (`sync.md` §2.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerConsent {
    /// The indexer URL.
    pub url: url::Url,
}

/// Engine configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// Production or devnet.
    pub network: Network,
    /// `sources`.
    pub sources: Sources,
    /// `verify`.
    pub verify: VerifyMode,
    /// `network.doh`.
    pub doh: Vec<url::Url>,
    /// `network.dns` (native only; the browser always uses DoH).
    pub dns: DnsMode,
    /// `network.plc` (ignored in devnet mode, which carries its own).
    pub plc: url::Url,
    /// `network.relays`.
    pub relays: Vec<url::Url>,
    /// `network.jetstream`.
    pub jetstream: Vec<url::Url>,
    /// `network.constellation` (assisted only).
    pub constellation: Option<url::Url>,
    /// `network.slingshot` (hint-only identity fallback; assisted only).
    pub slingshot: Option<url::Url>,
    /// User-configured or consented indexers.
    pub indexers: Vec<IndexerConsent>,
    /// `User-Agent` for native hosts.
    pub user_agent: String,
    /// Native or browser.
    pub platform: Platform,
    /// Parity mode (`engine-api.md` §2.1), **test-only**: when set, every view is computed at
    /// this fixed time (ms) instead of the host clock, and every admitted version's first-seen
    /// time (`obs`) is taken to be this time, so clients that synced the same records at
    /// different times produce byte-identical DTOs. Native clients set it only in devnet mode
    /// (`LESSDUGIN_PARITY_NOW_MS` with `--dev`), the SPA only in e2e builds; it is never set in
    /// production. Sync, the outbox, caches and back-offs keep using the host clock.
    pub parity_now_ms: Option<i64>,
}

fn url(s: &str) -> url::Url {
    url::Url::parse(s).expect("constant URL")
}

/// The default native `User-Agent` (`sync.md` §9.5).
pub fn default_user_agent() -> String {
    format!(
        "lessdugin/{} (+https://lessdugin.7778777.online)",
        env!("CARGO_PKG_VERSION")
    )
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            network: Network::Production,
            sources: Sources::Assisted,
            verify: VerifyMode::Pds,
            doh: DEFAULT_DOH.iter().map(|s| url(s)).collect(),
            dns: DnsMode::Doh,
            plc: url(DEFAULT_PLC),
            relays: vec![
                url("https://bsky.network"),
                url("https://relay1.us-east.bsky.network"),
            ],
            jetstream: vec![
                url("wss://jetstream.us-east.bsky.network/subscribe"),
                url("wss://jetstream.us-west.bsky.network/subscribe"),
            ],
            constellation: Some(url("https://constellation.microcosm.blue")),
            slingshot: Some(url(DEFAULT_SLINGSHOT)),
            indexers: Vec::new(),
            user_agent: default_user_agent(),
            platform: Platform::Native,
            parity_now_ms: None,
        }
    }
}

impl EngineConfig {
    /// Devnet mode against this PLC directory and PDS: `sources = direct`, no CORE+ services.
    pub fn devnet(plc: url::Url, pds: url::Url) -> Self {
        Self {
            network: Network::Devnet {
                plc: plc.clone(),
                pds,
            },
            sources: Sources::Direct,
            doh: Vec::new(),
            plc,
            relays: Vec::new(),
            jetstream: Vec::new(),
            constellation: None,
            slingshot: None,
            ..Self::default()
        }
    }

    /// True in devnet mode.
    pub fn is_devnet(&self) -> bool {
        matches!(self.network, Network::Devnet { .. })
    }

    /// The identity resolver's settings.
    pub fn identity_config(&self) -> IdentityConfig {
        match &self.network {
            Network::Devnet { plc, pds } => IdentityConfig::devnet(plc.clone(), pds.clone()),
            Network::Production => IdentityConfig {
                doh: self.doh.clone(),
                dns: match self.platform {
                    Platform::Browser => DnsMode::Doh,
                    Platform::Native => self.dns,
                },
                plc: self.plc.clone(),
                slingshot: self.slingshot.clone(),
                assisted: self.sources == Sources::Assisted,
                devnet_pds: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_devnet() {
        let c = EngineConfig::default();
        assert!(!c.is_devnet());
        let id = c.identity_config();
        assert_eq!(id.doh.len(), 2);
        assert_eq!(id.doh[0].as_str(), "https://cloudflare-dns.com/dns-query");
        assert_eq!(id.plc.as_str(), "https://plc.directory/");
        assert!(id.assisted && id.slingshot.is_some() && !id.is_devnet());
        assert!(c.user_agent.starts_with("lessdugin/"));

        let browser = EngineConfig {
            platform: Platform::Browser,
            dns: DnsMode::System,
            ..c
        };
        assert_eq!(browser.identity_config().dns, DnsMode::Doh);

        let d = EngineConfig::devnet(url("http://localhost:2582"), url("http://localhost:2583"));
        assert!(d.is_devnet());
        let id = d.identity_config();
        assert!(id.is_devnet() && !id.assisted && id.slingshot.is_none());
        assert_eq!(id.plc.as_str(), "http://localhost:2582/");
    }
}
