// SPDX-License-Identifier: AGPL-3.0-or-later
//! The worker's engine configuration (`EngineConfigInput` in `packages/engine/src/protocol.ts`)
//! → [`EngineConfig`] for the browser platform (`engine-api.md` §2).
//!
//! Every field is optional; absent fields keep the engine's defaults. `network.kind = "devnet"`
//! and `parityNowMs` are test-only: the worker passes them only from an e2e or development build
//! (threat-model INFRA-9, `engine-api.md` §2.1); this module accepts what it is given.

use lessdugin_core::engine::{
    EngineConfig, EngineError, IndexerConsent, Platform, Sources, VerifyMode,
};
use serde::Deserialize;

/// The configuration JSON.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigInput {
    /// Production (default) or a devnet.
    #[serde(default)]
    pub network: Option<NetworkInput>,
    /// `"direct"` or `"assisted"`.
    #[serde(default)]
    pub sources: Option<String>,
    /// `"pds"` or `"proof"`.
    #[serde(default)]
    pub verify: Option<String>,
    /// DoH resolvers.
    #[serde(default)]
    pub doh: Option<Vec<String>>,
    /// The PLC directory.
    #[serde(default)]
    pub plc: Option<String>,
    /// Relays.
    #[serde(default)]
    pub relays: Option<Vec<String>>,
    /// Jetstream endpoints.
    #[serde(default)]
    pub jetstream: Option<Vec<String>>,
    /// Constellation (`null` disables it).
    #[serde(default, deserialize_with = "some_nullable")]
    pub constellation: Option<Option<String>>,
    /// Slingshot (`null` disables it).
    #[serde(default, deserialize_with = "some_nullable")]
    pub slingshot: Option<Option<String>>,
    /// Consented indexers.
    #[serde(default)]
    pub indexers: Option<Vec<String>>,
    /// Parity mode (test-only, `engine-api.md` §2.1).
    #[serde(default)]
    pub parity_now_ms: Option<i64>,
}

/// `network`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NetworkInput {
    /// The public network.
    Production,
    /// A local devnet (ADR 0006).
    Devnet {
        /// The devnet PLC directory.
        plc: String,
        /// The devnet PDS.
        pds: String,
    },
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn some_nullable<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

fn url(field: &str, s: &str) -> Result<url::Url, EngineError> {
    url::Url::parse(s)
        .map_err(|e| EngineError::invalid_input(format!("config.{field}: {s:?}: {e}")))
}

fn urls(field: &str, list: &[String]) -> Result<Vec<url::Url>, EngineError> {
    list.iter().map(|s| url(field, s)).collect()
}

/// Parses the configuration JSON (`""` or `"null"`: the defaults).
pub fn parse(json: &str) -> Result<EngineConfig, EngineError> {
    let input: ConfigInput = match json.trim() {
        "" | "null" => ConfigInput::default(),
        s if !s.starts_with('{') => {
            return Err(EngineError::invalid_input(
                "engine configuration: not a JSON object",
            ));
        }
        s => serde_json::from_str(s)
            .map_err(|e| EngineError::invalid_input(format!("engine configuration: {e}")))?,
    };
    build(input)
}

/// Builds the browser [`EngineConfig`].
pub fn build(input: ConfigInput) -> Result<EngineConfig, EngineError> {
    let mut c = match &input.network {
        None | Some(NetworkInput::Production) => EngineConfig::default(),
        Some(NetworkInput::Devnet { plc, pds }) => {
            EngineConfig::devnet(url("network.plc", plc)?, url("network.pds", pds)?)
        }
    };
    c.platform = Platform::Browser;
    if let Some(s) = &input.sources {
        c.sources = match s.as_str() {
            "direct" => Sources::Direct,
            "assisted" => Sources::Assisted,
            _ => return Err(EngineError::invalid_input(format!("config.sources: {s:?}"))),
        };
    }
    if let Some(v) = &input.verify {
        c.verify = match v.as_str() {
            "pds" => VerifyMode::Pds,
            "proof" => VerifyMode::Proof,
            _ => return Err(EngineError::invalid_input(format!("config.verify: {v:?}"))),
        };
    }
    if let Some(l) = &input.doh {
        c.doh = urls("doh", l)?;
    }
    if let Some(p) = &input.plc {
        // devnet mode carries its own PLC
        if !c.is_devnet() {
            c.plc = url("plc", p)?;
        }
    }
    if let Some(l) = &input.relays {
        c.relays = urls("relays", l)?;
    }
    if let Some(l) = &input.jetstream {
        c.jetstream = urls("jetstream", l)?;
    }
    if let Some(v) = &input.constellation {
        c.constellation = v.as_deref().map(|s| url("constellation", s)).transpose()?;
    }
    if let Some(v) = &input.slingshot {
        c.slingshot = v.as_deref().map(|s| url("slingshot", s)).transpose()?;
    }
    if let Some(l) = &input.indexers {
        c.indexers = urls("indexers", l)?
            .into_iter()
            .map(|url| IndexerConsent { url })
            .collect();
    }
    c.parity_now_ms = input.parity_now_ms;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_browser_production_config() {
        let c = parse("").unwrap();
        assert_eq!(c.platform, Platform::Browser);
        assert!(!c.is_devnet());
        assert_eq!(c.parity_now_ms, None);
        assert_eq!(
            EngineConfig {
                platform: Platform::Native,
                ..c
            },
            EngineConfig::default()
        );
        assert_eq!(parse("null").unwrap().platform, Platform::Browser);
    }

    #[test]
    fn devnet_parity_and_overrides() {
        let c = parse(
            r#"{"network":{"kind":"devnet","plc":"http://localhost:2582","pds":"http://localhost:2583"},
                "parityNowMs":1700000000000,"slingshot":null,"sources":"direct"}"#,
        )
        .unwrap();
        assert!(c.is_devnet());
        assert_eq!(c.plc.as_str(), "http://localhost:2582/");
        assert_eq!(c.parity_now_ms, Some(1_700_000_000_000));
        assert_eq!(c.slingshot, None);
        assert_eq!(c.sources, Sources::Direct);
        assert_eq!(c.platform, Platform::Browser);

        let c = parse(r#"{"doh":["https://dns.example/dns-query"],"constellation":null}"#).unwrap();
        assert_eq!(c.doh.len(), 1);
        assert_eq!(c.constellation, None);
        assert!(c.slingshot.is_some());
    }

    #[test]
    fn rejects_unknown_fields_and_bad_values() {
        for bad in [
            r#"{"parity":1}"#,
            r#"{"sources":"all"}"#,
            r#"{"doh":["not a url"]}"#,
            r#"{"network":{"kind":"testnet"}}"#,
            "[]",
        ] {
            let e = parse(bad).unwrap_err();
            assert_eq!(e.code.as_str(), "invalid_input", "{bad}");
        }
    }
}
