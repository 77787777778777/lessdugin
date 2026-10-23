// SPDX-License-Identifier: AGPL-3.0-or-later
//! `web.port` in `config.toml` (`threat-model.md` WEB-2):
//!
//! ```toml
//! [web]
//! port = 47778
//! ```
//!
//! The port is part of the bridge's origin (and so of the browser's storage for it), so it is a
//! setting rather than a per-run choice; `--port` overrides it for one run. The table is read
//! here, from the same file `Config::load` read, so that the native `Config` type does not need
//! to know about it.

use std::path::Path;

use super::policy::WEB_PORT_DEFAULT;

/// Largest `config.toml` read (as `lessdugin_native::config::CONFIG_MAX_BYTES`).
const CONFIG_MAX_BYTES: u64 = lessdugin_native::config::CONFIG_MAX_BYTES;

/// `web.port` from TOML text: `Ok(None)` when absent, an error when it is not a port.
pub fn port_from_toml(text: &str) -> Result<Option<u16>, String> {
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        // An unreadable configuration is reported (and ignored) by `Ctx::new`.
        Err(_) => return Ok(None),
    };
    let Some(web) = table.get("web") else {
        return Ok(None);
    };
    let Some(web) = web.as_table() else {
        return Err("web in config.toml must be a table".into());
    };
    match web.get("port") {
        None => Ok(None),
        Some(v) => v
            .as_integer()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .map(Some)
            .ok_or_else(|| "web.port in config.toml must be a port number (1-65535)".into()),
    }
}

/// The port to listen on: `--port`, else `web.port` from `config_file`, else 47778.
pub fn port(flag: Option<u16>, config_file: &Path) -> Result<u16, String> {
    if let Some(p) = flag {
        return Ok(p);
    }
    let text = match std::fs::metadata(config_file) {
        Ok(m) if m.len() <= CONFIG_MAX_BYTES => std::fs::read_to_string(config_file).ok(),
        _ => None,
    };
    Ok(text
        .as_deref()
        .map(port_from_toml)
        .transpose()?
        .flatten()
        .unwrap_or(WEB_PORT_DEFAULT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_port_setting() {
        assert_eq!(port_from_toml(""), Ok(None));
        assert_eq!(port_from_toml("sources = \"direct\"\n"), Ok(None));
        assert_eq!(port_from_toml("[web]\nport = 48000\n"), Ok(Some(48000)));
        assert_eq!(port_from_toml("[web]\n"), Ok(None));
        assert!(port_from_toml("[web]\nport = 0\n").is_err());
        assert!(port_from_toml("[web]\nport = 70000\n").is_err());
        assert!(port_from_toml("[web]\nport = \"80\"\n").is_err());
        assert!(port_from_toml("web = 3\n").is_err());
        // Not TOML: left to Ctx::new, which warns and uses the defaults.
        assert_eq!(port_from_toml("[[[["), Ok(None));
    }

    #[test]
    fn flag_wins_then_file_then_default() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("config.toml");
        assert_eq!(port(None, &f), Ok(WEB_PORT_DEFAULT));
        std::fs::write(&f, "[web]\nport = 48001\n").unwrap();
        assert_eq!(port(None, &f), Ok(48001));
        assert_eq!(port(Some(48002), &f), Ok(48002));
    }
}
