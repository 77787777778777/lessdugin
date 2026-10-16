// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fault injection, listing behaviours, equivocation selectors and per-host rate limits.

use std::collections::BTreeSet;

use lessdugin_core::host::HttpResponse;

/// What a matching request gets instead of (or on top of) the normal answer.
#[derive(Clone, Debug)]
pub enum Fault {
    /// Answers after this many milliseconds. Over [`super::FakeNet::install`] the attached
    /// `MemHost`'s fake clock advances by that much (a request whose timeout is shorter fails with
    /// `HostError::Timeout` after the timeout has passed); over HTTP the server sleeps. Latency
    /// stacks with the other faults.
    Latency(u64),
    /// This status with an XRPC error body (`InternalServerError` for 5xx, `InvalidRequest`
    /// otherwise).
    Status(u16),
    /// This status and XRPC error.
    Error {
        /// HTTP status.
        status: u16,
        /// XRPC error name.
        error: String,
        /// Message.
        message: String,
    },
    /// HTTP 429 `RateLimitExceeded` with `Retry-After: <seconds>`.
    RateLimited {
        /// `Retry-After` in seconds.
        retry_after_s: u64,
    },
    /// The normal answer, with the body cut to this many bytes. Over `MemHost` the short body is
    /// returned as is; over HTTP the full `Content-Length` is announced and the connection is
    /// dropped after these bytes.
    Truncate(usize),
    /// The connection drops before an answer (`HostError::Network` over `MemHost`; an aborted
    /// response over HTTP).
    Disconnect,
    /// A canned response.
    Respond(HttpResponse),
}

impl Fault {
    /// [`Fault::Status`].
    pub fn status(status: u16) -> FaultRule {
        FaultRule::new(Self::Status(status))
    }

    /// [`Fault::Error`].
    pub fn error(status: u16, error: &str, message: &str) -> FaultRule {
        FaultRule::new(Self::Error {
            status,
            error: error.to_owned(),
            message: message.to_owned(),
        })
    }

    /// [`Fault::RateLimited`].
    pub fn rate_limited(retry_after_s: u64) -> FaultRule {
        FaultRule::new(Self::RateLimited { retry_after_s })
    }

    /// [`Fault::Latency`].
    pub fn latency(ms: u64) -> FaultRule {
        FaultRule::new(Self::Latency(ms))
    }

    /// [`Fault::Truncate`].
    pub fn truncate(bytes: usize) -> FaultRule {
        FaultRule::new(Self::Truncate(bytes))
    }

    /// [`Fault::Disconnect`].
    pub fn disconnect() -> FaultRule {
        FaultRule::new(Self::Disconnect)
    }

    /// [`Fault::Respond`].
    pub fn respond(resp: HttpResponse) -> FaultRule {
        FaultRule::new(Self::Respond(resp))
    }

    /// True for faults that replace the answer.
    pub(crate) fn is_terminal(&self) -> bool {
        !matches!(self, Self::Latency(_) | Self::Truncate(_))
    }
}

/// A fault and the requests it applies to. Every set field must match; unset fields match
/// anything.
#[derive(Clone, Debug)]
pub struct FaultRule {
    /// The fault.
    pub fault: Fault,
    /// Service base URL (a PDS, the PLC directory or a relay), e.g. `https://pds.example.com`.
    pub host: Option<String>,
    /// Subject repo DID (from `did`/`repo` parameters, the body's `repo`, a `userpds://` target,
    /// or a PLC document path).
    pub did: Option<String>,
    /// XRPC method NSID (`com.atproto.repo.listRecords`), or `plc` for PLC document requests.
    pub nsid: Option<String>,
    /// Caller label ([`super::FakeNet::install_as`]; `x-fake-caller` header over HTTP).
    pub caller: Option<String>,
    /// Matching requests to let through before the fault starts.
    pub skip: u32,
    /// How many matching requests get the fault (`None`: every one).
    pub times: Option<u32>,
}

impl FaultRule {
    /// A rule matching every request.
    pub fn new(fault: Fault) -> Self {
        Self {
            fault,
            host: None,
            did: None,
            nsid: None,
            caller: None,
            skip: 0,
            times: None,
        }
    }

    /// Only requests to this service base URL.
    pub fn on_host(mut self, base: &str) -> Self {
        self.host = Some(base.trim_end_matches('/').to_owned());
        self
    }

    /// Only requests about this repo.
    pub fn on_repo(mut self, did: &str) -> Self {
        self.did = Some(did.to_owned());
        self
    }

    /// Only this XRPC method (or `plc`).
    pub fn on_nsid(mut self, nsid: &str) -> Self {
        self.nsid = Some(nsid.to_owned());
        self
    }

    /// Only requests from this caller.
    pub fn for_caller(mut self, caller: &str) -> Self {
        self.caller = Some(caller.to_owned());
        self
    }

    /// Lets the first `n` matching requests through.
    pub fn after(mut self, n: u32) -> Self {
        self.skip = n;
        self
    }

    /// Applies to `n` matching requests, then expires.
    pub fn times(mut self, n: u32) -> Self {
        self.times = Some(n);
        self
    }

    /// Applies once.
    pub fn once(self) -> Self {
        self.times(1)
    }

    pub(crate) fn matches(&self, m: &MatchKey<'_>) -> bool {
        self.host.as_deref().is_none_or(|h| h == m.host)
            && self.did.as_deref().is_none_or(|d| Some(d) == m.did)
            && self.nsid.as_deref().is_none_or(|n| Some(n) == m.nsid)
            && self.caller.as_deref().is_none_or(|c| c == m.caller)
    }
}

pub(crate) struct MatchKey<'a> {
    pub host: &'a str,
    pub did: Option<&'a str>,
    pub nsid: Option<&'a str>,
    pub caller: &'a str,
}

/// An injected fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FaultId(pub(crate) u64);

pub(crate) struct ActiveFault {
    pub id: FaultId,
    pub rule: FaultRule,
    pub seen: u32,
    pub applied: u32,
}

impl ActiveFault {
    /// Counts a matching request; true if the fault applies to it.
    pub fn hit(&mut self) -> bool {
        self.seen += 1;
        if self.seen <= self.rule.skip {
            return false;
        }
        if self.rule.times.is_some_and(|t| self.applied >= t) {
            return false;
        }
        self.applied += 1;
        true
    }

    pub fn expired(&self) -> bool {
        self.rule.times.is_some_and(|t| self.applied >= t)
    }
}

/// How `com.atproto.repo.listRecords` treats cursors (ADR 0004).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorMode {
    /// The reference PDS: the cursor is an exclusive rkey bound (any string, compared bytewise),
    /// and a non-empty page's cursor is its last key.
    #[default]
    Bound,
    /// Cursors are random tokens the server remembers. An unknown cursor (such as `{fk}.`) lists
    /// from the start.
    Opaque,
    /// Like [`CursorMode::Opaque`], but an unknown cursor is a `400 InvalidRequest`.
    OpaqueStrict,
    /// A cursor that is not the rkey of an existing record in the collection is ignored (the
    /// listing starts at the top); cursors the server issued still page correctly.
    IgnoresBound,
    /// A broken server: every page returns the request's cursor again (the first page returns its
    /// last key), so a client that follows cursors loops.
    Stuck,
}

/// Which callers see an equivocating repo's fork.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForkSelector {
    /// Every caller.
    All,
    /// Callers with these labels.
    Callers(BTreeSet<String>),
    /// Every other request about the repo (the first sees the fork).
    Alternate,
}

impl ForkSelector {
    /// [`ForkSelector::Callers`].
    pub fn callers<I: IntoIterator<Item = S>, S: Into<String>>(callers: I) -> Self {
        Self::Callers(callers.into_iter().map(Into::into).collect())
    }
}

/// A per-host read budget like Bluesky's (`ratelimit-*` headers; 429 when exhausted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// Requests per window.
    pub limit: u32,
    /// Window length (ms).
    pub window_ms: i64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RateState {
    pub cfg: RateLimit,
    pub window_start: i64,
    pub used: u32,
}

impl RateState {
    pub fn new(cfg: RateLimit, now: i64) -> Self {
        Self {
            cfg,
            window_start: now,
            used: 0,
        }
    }

    /// Counts a request at `now`; returns `(allowed, headers)`.
    pub fn take(&mut self, now: i64) -> (bool, Vec<(String, String)>) {
        if now >= self.window_start + self.cfg.window_ms {
            self.window_start = now;
            self.used = 0;
        }
        let allowed = self.used < self.cfg.limit;
        if allowed {
            self.used += 1;
        }
        let reset_s = (self.window_start + self.cfg.window_ms).div_euclid(1000);
        let headers = vec![
            ("ratelimit-limit".to_owned(), self.cfg.limit.to_string()),
            (
                "ratelimit-remaining".to_owned(),
                (self.cfg.limit - self.used).to_string(),
            ),
            ("ratelimit-reset".to_owned(), reset_s.to_string()),
            (
                "ratelimit-policy".to_owned(),
                format!("{};w={}", self.cfg.limit, self.cfg.window_ms / 1000),
            ),
        ];
        (allowed, headers)
    }
}
