// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /bridge/lock`: the native `outbox:<did>` lock, held for a tab (`threat-model.md` §7.4,
//! `sync.md` §7.1, §10.6).
//!
//! In `lessdugin web` the tab's outbox rows live in the native `state.sqlite` (`/bridge/kv`), so
//! the tab and native clients (`lessdugin sync`, `outbox flush`, the TUI) pump the same rows. A
//! browser Web Lock does not exclude a native file lock, so without this endpoint both could
//! send the same write ("one write in flight per account", sync.md §7.1). A tab in bridge mode
//! therefore takes its Web Lock `lessdugin-outbox-<did>` and then this lock, for as long as it
//! holds the account's outbox:
//!
//! ```text
//! POST /bridge/lock {"name": "outbox:<did>", "wait": bool}
//!   name is not outbox:<valid DID>            → 400 invalid_input
//!   the native lock is taken (waiting at most BRIDGE_LOCK_WAIT_MS when `wait`):
//!     taken     → 200, Content-Type application/x-ndjson, a body that does not end:
//!                 {"held":true}\n, then \n every BRIDGE_LOCK_HEARTBEAT_MS
//!     not taken → 200 {"held":false} (busy, or the wait ran out: ask again)
//! ```
//!
//! The lock is held exactly as long as the response body is open: the tab releases it by
//! aborting the request (`AbortController`), and a tab that closes or crashes releases it with
//! its connection (the heartbeat's write fails within about two beats). The body also ends,
//! releasing the lock, when the tab's bridge session is revoked (§7.2) or the server stops; a
//! revoked tab cannot send writes anyway (`/bridge/xrpc` answers 401).

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::Host as _;
use serde_json::json;

use super::policy::{BRIDGE_LOCK_HEARTBEAT_MS, BRIDGE_LOCK_WAIT_MS};
use super::server::{Web, engine_error, error, json_response};

/// The first line of a held lock's body.
pub const HELD_LINE: &[u8] = b"{\"held\":true}\n";

/// The lock name a tab may take, if `name` is one: `outbox:<did>` with a valid DID.
pub fn lock_name(name: &str) -> Option<String> {
    let did = name.strip_prefix("outbox:")?;
    let did = Did::parse(did).ok()?;
    Some(format!("outbox:{did}"))
}

/// Handles one `/bridge/lock` request (it already passed the §7.3 checks) for the session
/// `token`.
pub async fn handle(web: &Arc<Web>, token: &str, body: &[u8]) -> Response {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Req {
        name: String,
        #[serde(default)]
        wait: bool,
    }
    let req = match serde_json::from_slice::<Req>(body) {
        Ok(r) => r,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "expected {\"name\": \"outbox:<did>\", \"wait\": true | false}",
            );
        }
    };
    let Some(name) = lock_name(&req.name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "only outbox:<did> locks are bridged",
        );
    };
    let host = web.engine.host();
    let taken = if req.wait {
        match tokio::time::timeout(
            Duration::from_millis(BRIDGE_LOCK_WAIT_MS),
            host.lock(&name, true),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Ok(None),
        }
    } else {
        host.lock(&name, false).await
    };
    let guard = match taken {
        Ok(Some(g)) => g,
        Ok(None) => return json_response(StatusCode::OK, &json!({"held": false})),
        Err(e) => return engine_error(&e.into()),
    };
    let held = Held {
        _guard: guard,
        web: web.clone(),
        token: token.to_owned(),
        shutdown: web.shutdown.subscribe(),
        first: true,
    };
    let stream = futures::stream::unfold(held, |mut held| async move {
        if held.first {
            held.first = false;
            return Some((Ok::<_, std::io::Error>(Bytes::from_static(HELD_LINE)), held));
        }
        let beat = tokio::time::sleep(Duration::from_millis(BRIDGE_LOCK_HEARTBEAT_MS));
        tokio::select! {
            () = beat => {}
            _ = held.shutdown.changed() => return None,
        }
        if *held.shutdown.borrow() || !held.web.caps.is_session(&held.token) {
            // The server stops, or the tab's session was revoked: release.
            return None;
        }
        Some((Ok(Bytes::from_static(b"\n")), held))
    });
    let mut r = (StatusCode::OK, Body::from_stream(stream)).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    r
}

/// A held lock and what ends it.
struct Held {
    _guard: lessdugin_native::host::NativeLock,
    web: Arc<Web>,
    token: String,
    shutdown: tokio::sync::watch::Receiver<bool>,
    first: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_outbox_locks_of_valid_dids() {
        assert_eq!(
            lock_name("outbox:did:plc:ewvi7nxzyoun6zhxrhs64oiz").as_deref(),
            Some("outbox:did:plc:ewvi7nxzyoun6zhxrhs64oiz")
        );
        for bad in [
            "outbox:",
            "outbox:alice.test",
            "outbox:did:plc:x/../../y",
            "refresh:did:plc:ewvi7nxzyoun6zhxrhs64oiz",
            "repo:did:plc:ewvi7nxzyoun6zhxrhs64oiz",
            "sync-leader",
            "",
        ] {
            assert!(lock_name(bad).is_none(), "{bad}");
        }
    }
}
