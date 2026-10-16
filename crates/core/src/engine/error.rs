// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`EngineError`] (`engine-api.md` §1): `{ "code", "message", "retryable", "detail"? }`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
#[cfg(feature = "tsify")]
use tsify::Tsify;

use crate::host::HostError;
use crate::identity::IdentityError;
use crate::xrpc::XrpcCallError;

/// Error codes (`engine-api.md` §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "tsify", derive(Tsify))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No usable session; log in.
    AuthRequired,
    /// The record, repo, actor or account does not exist.
    NotFound,
    /// The reference could not be parsed or resolved.
    InvalidRef,
    /// Invalid input (including requests a server rejected as invalid).
    InvalidInput,
    /// A precondition failed (swap, login binding, pin change needing confirmation).
    Conflict,
    /// Rate limited.
    RateLimited,
    /// Network failure.
    Network,
    /// Refused by policy (SSRF guard, CORS, redirects, login-binding violations).
    Blocked,
    /// A response or input exceeded a cap.
    TooLarge,
    /// Results incomplete under the budget; `detail` carries the partial result.
    Partial,
    /// Not supported here.
    Unsupported,
    /// A bug.
    Internal,
}

impl ErrorCode {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::NotFound => "not_found",
            Self::InvalidRef => "invalid_ref",
            Self::InvalidInput => "invalid_input",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Network => "network",
            Self::Blocked => "blocked",
            Self::TooLarge => "too_large",
            Self::Partial => "partial",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        }
    }

    /// The CLI exit code (`engine-api.md` §1; usage errors (2) are decided by the CLI).
    pub fn exit_code(self) -> i32 {
        match self {
            Self::AuthRequired => 3,
            Self::NotFound => 4,
            Self::Conflict => 5,
            Self::Partial => 6,
            _ => 1,
        }
    }

    /// Whether errors with this code are retryable by default.
    pub fn default_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Network)
    }
}

/// An engine error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "tsify", derive(Tsify))]
#[error("{message}")]
pub struct EngineError {
    /// The code.
    pub code: ErrorCode,
    /// A human-readable message (never contains credentials; may quote server text, which
    /// terminals must sanitize).
    pub message: String,
    /// Whether retrying later may succeed.
    pub retryable: bool,
    /// Structured detail (a JSON object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl EngineError {
    /// An error with the code's default retryability.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.default_retryable(),
            detail: None,
        }
    }

    /// Sets `retryable`.
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Adds a field to `detail` (an object).
    pub fn with_detail(mut self, key: &str, value: impl Into<Value>) -> Self {
        let mut map = match self.detail.take() {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        map.insert(key.to_owned(), value.into());
        self.detail = Some(Value::Object(map));
        self
    }

    /// Prefixes the message with context.
    pub fn context(mut self, ctx: &str) -> Self {
        self.message = format!("{ctx}: {}", self.message);
        self
    }

    /// `not_found`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// `invalid_ref`.
    pub fn invalid_ref(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRef, message)
    }

    /// `invalid_input`.
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    /// `conflict`.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// `blocked`.
    pub fn blocked(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Blocked, message)
    }

    /// `internal`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// The CLI exit code.
    pub fn exit_code(&self) -> i32 {
        self.code.exit_code()
    }
}

impl From<XrpcCallError> for EngineError {
    fn from(e: XrpcCallError) -> Self {
        use XrpcCallError as X;
        let message = e.to_string();
        let mut err = match &e {
            X::RateLimited { retry_after_ms } => {
                let err = Self::new(ErrorCode::RateLimited, message);
                match retry_after_ms {
                    Some(ms) => err.with_detail("retryAfterMs", json!(ms)),
                    None => err,
                }
            }
            X::InvalidSwap { .. } => Self::conflict(message),
            X::RecordNotFound
            | X::RepoNotFound
            | X::RepoDeactivated
            | X::RepoTakendown
            | X::RepoSuspended => Self::not_found(message),
            X::AuthRequired { .. } => Self::new(ErrorCode::AuthRequired, message),
            X::Xrpc { status, .. } | X::Http(status) if *status >= 500 => {
                Self::new(ErrorCode::Network, message)
            }
            X::Http(404 | 410) => Self::not_found(message),
            X::Http(s) if (300..400).contains(s) => {
                Self::blocked(format!("{message} (redirects are not followed)"))
            }
            X::Xrpc { .. } | X::Http(_) | X::InvalidRequest(_) => Self::invalid_input(message),
            X::Network(_) | X::Timeout => Self::new(ErrorCode::Network, message),
            X::TooLarge => Self::new(ErrorCode::TooLarge, message),
            X::Blocked(_) | X::Cors => Self::blocked(message),
            X::Unsupported(_) => Self::new(ErrorCode::Unsupported, message),
            X::Decode(_) => Self::new(ErrorCode::Network, message).with_retryable(false),
            X::Other(_) => Self::internal(message),
        };
        if let Some(name) = e.error_name() {
            err = err.with_detail("error", name);
        }
        if let Some(status) = e.status() {
            err = err.with_detail("status", status);
        }
        err
    }
}

impl From<IdentityError> for EngineError {
    fn from(e: IdentityError) -> Self {
        let message = e.to_string();
        match e {
            IdentityError::InvalidInput(_)
            | IdentityError::UnsupportedDid { .. }
            | IdentityError::InvalidHandle { .. } => Self::invalid_ref(message),
            IdentityError::HandleNotFound(_) | IdentityError::DidNotFound(_) => {
                Self::not_found(message)
            }
            IdentityError::HandleUnreachable(_) => Self::new(ErrorCode::Network, message),
            IdentityError::InvalidDoc { .. } => {
                Self::not_found(message).with_detail("reason", "invalidDidDocument")
            }
            IdentityError::Fetch { did, error } => {
                Self::from(error).context(&format!("DID document of {did}"))
            }
        }
    }
}

impl From<HostError> for EngineError {
    fn from(e: HostError) -> Self {
        match e {
            HostError::Storage(m) => Self::internal(format!("storage error: {m}")),
            other => Self::from(XrpcCallError::from_host(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape() {
        let e = EngineError::not_found("no such record").with_detail("uri", "at://x");
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"code":"not_found","message":"no such record","retryable":false,"detail":{"uri":"at://x"}}"#
        );
        let n = EngineError::new(ErrorCode::Network, "down");
        assert_eq!(
            serde_json::to_string(&n).unwrap(),
            r#"{"code":"network","message":"down","retryable":true}"#
        );
        let back: EngineError = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        for c in [
            ErrorCode::AuthRequired,
            ErrorCode::TooLarge,
            ErrorCode::InvalidRef,
        ] {
            assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        }
        assert_eq!(ErrorCode::AuthRequired.exit_code(), 3);
        assert_eq!(ErrorCode::NotFound.exit_code(), 4);
        assert_eq!(ErrorCode::Conflict.exit_code(), 5);
        assert_eq!(ErrorCode::Partial.exit_code(), 6);
        assert_eq!(ErrorCode::Network.exit_code(), 1);
    }

    #[test]
    fn from_xrpc() {
        let e = EngineError::from(XrpcCallError::RateLimited {
            retry_after_ms: Some(1500),
        });
        assert_eq!(e.code, ErrorCode::RateLimited);
        assert!(e.retryable);
        assert_eq!(e.detail.as_ref().unwrap()["retryAfterMs"], 1500);
        assert_eq!(e.detail.as_ref().unwrap()["status"], 429);
        let e = EngineError::from(XrpcCallError::InvalidSwap { message: None });
        assert_eq!((e.code, e.retryable), (ErrorCode::Conflict, false));
        assert_eq!(
            EngineError::from(XrpcCallError::RepoTakendown).code,
            ErrorCode::NotFound
        );
        assert_eq!(
            EngineError::from(XrpcCallError::Http(503)).code,
            ErrorCode::Network
        );
        assert!(EngineError::from(XrpcCallError::Http(503)).retryable);
        assert_eq!(
            EngineError::from(XrpcCallError::Http(302)).code,
            ErrorCode::Blocked
        );
        assert_eq!(
            EngineError::from(XrpcCallError::Http(404)).code,
            ErrorCode::NotFound
        );
        assert_eq!(
            EngineError::from(XrpcCallError::Cors).code,
            ErrorCode::Blocked
        );
        assert_eq!(
            EngineError::from(XrpcCallError::TooLarge).code,
            ErrorCode::TooLarge
        );
        assert_eq!(
            EngineError::from(XrpcCallError::AuthRequired {
                error: None,
                message: None
            })
            .code,
            ErrorCode::AuthRequired
        );
        assert_eq!(
            EngineError::from(HostError::Storage("disk".into())).code,
            ErrorCode::Internal
        );
        // A blob that fails its CID check is refused data, not a bug.
        let e = EngineError::from(HostError::CidMismatch);
        assert_eq!((e.code, e.retryable), (ErrorCode::Blocked, false));
        // No reachable handle resolver is a network failure, not "not found".
        let e = EngineError::from(IdentityError::HandleUnreachable("alice.example.com".into()));
        assert_eq!((e.code, e.retryable), (ErrorCode::Network, true));
        assert_eq!(
            EngineError::from(IdentityError::HandleNotFound("alice.example.com".into())).code,
            ErrorCode::NotFound
        );
    }
}
