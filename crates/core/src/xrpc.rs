// SPDX-License-Identifier: AGPL-3.0-or-later
//! A small XRPC client over [`Host`] (`sync.md` §9.3, §9.5, §12.1).
//!
//! - URLs are built with the `lessdugin_atp::xrpc` helpers (RFC 3986 percent-encoded parameters).
//! - Non-2xx responses and host failures map to a typed [`XrpcCallError`]: XRPC error bodies
//!   `{"error", "message"}` are recognised by name, anything else by HTTP status.
//! - GETs are retried ([`RetryPolicy`]: 3 retries, full-jitter exponential backoff of
//!   `uniform(0, min(8 s, 250 ms · 2^n))`, at least `Retry-After`) on network errors, timeouts,
//!   5xx and 429. Procedures are never retried by the transport (`sync.md` §7.2). Waiting uses
//!   [`Host::sleep`] and jitter comes from [`Host::fill_random`]: the core reads no clock itself.
//! - Every call carries an explicit byte cap and timeout ([`CallOpts`], `crate::limits`).
//! - Anonymous GETs carry no custom headers; at most a CORS-safelisted `accept`, so browsers send
//!   no preflight. Request bodies and credentials are never logged.

use lessdugin_atp::syntax::Did;
use lessdugin_atp::xrpc::{Endpoint, Method, QueryParams, XrpcError, procedure_path, query_path};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::host::{Host, HostError, HttpRequest, HttpResponse, Redirects, Target};
use crate::limits;

/// Where a call goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Service {
    /// An anonymous call to the service at this base URL (`https://pds.example.com`). A path in
    /// the base is kept (`https://example.com/api` + `/xrpc/…`).
    Anon(url::Url),
    /// An authenticated call to the PDS bound to this account at login (the host adds auth and
    /// never follows redirects).
    UserPds(Did),
}

impl Service {
    /// The request target for `path_and_query` (which starts with `/`).
    pub fn target(&self, path_and_query: &str) -> Result<Target, XrpcCallError> {
        match self {
            Self::Anon(base) => {
                let s = format!("{}{}", base.as_str().trim_end_matches('/'), path_and_query);
                url::Url::parse(&s)
                    .map(Target::Anon)
                    .map_err(|e| XrpcCallError::InvalidRequest(format!("bad URL: {e}")))
            }
            Self::UserPds(did) => Ok(Target::UserPds {
                account: did.clone(),
                path_and_query: path_and_query.to_owned(),
            }),
        }
    }
}

/// Per-call options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallOpts {
    /// Response byte cap.
    pub max_bytes: u64,
    /// Total timeout per attempt.
    pub timeout_ms: u32,
    /// Redirect policy (default: none, as every `pds`-level read requires).
    pub redirects: Redirects,
    /// Retry transient failures (GET only; ignored for procedures).
    pub retry: bool,
    /// A CORS-safelisted `accept` value, if any.
    pub accept: Option<&'static str>,
}

impl Default for CallOpts {
    fn default() -> Self {
        Self {
            max_bytes: limits::DEFAULT_RESPONSE_MAX_BYTES,
            timeout_ms: limits::REQUEST_TIMEOUT_MS,
            redirects: Redirects::None,
            retry: true,
            accept: None,
        }
    }
}

impl CallOpts {
    /// Defaults with this byte cap.
    pub fn capped(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            ..Self::default()
        }
    }

    /// Sets the timeout.
    pub fn with_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Sets the redirect policy.
    pub fn with_redirects(mut self, redirects: Redirects) -> Self {
        self.redirects = redirects;
        self
    }

    /// Disables transport retries.
    pub fn without_retry(mut self) -> Self {
        self.retry = false;
        self
    }

    /// Sets the `accept` header.
    pub fn with_accept(mut self, accept: &'static str) -> Self {
        self.accept = Some(accept);
        self
    }
}

/// Retry policy for GETs (`sync.md` §9.3, *tunable*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Backoff base (ms).
    pub base_ms: u64,
    /// Backoff cap (ms).
    pub cap_ms: u64,
    /// The longest `Retry-After` honoured by waiting inside the call; longer ones end the call
    /// with [`XrpcCallError::RateLimited`].
    pub max_retry_after_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: limits::RETRY_MAX,
            base_ms: limits::RETRY_BASE_MS,
            cap_ms: limits::RETRY_CAP_MS,
            max_retry_after_ms: limits::RETRY_AFTER_MAX_WAIT_MS,
        }
    }
}

impl RetryPolicy {
    /// No retries.
    pub const NONE: Self = Self {
        max_retries: 0,
        base_ms: 0,
        cap_ms: 0,
        max_retry_after_ms: 0,
    };

    /// The wait before retry number `attempt` (0-based) given a random value:
    /// `max(uniform(0, min(cap, base · 2^attempt)), retry_after)`. `None` if `retry_after` is
    /// longer than [`RetryPolicy::max_retry_after_ms`].
    pub fn delay_ms(&self, attempt: u32, random: u64, retry_after_ms: Option<u64>) -> Option<u64> {
        let ceiling = self
            .base_ms
            .saturating_mul(1u64 << attempt.min(32))
            .min(self.cap_ms);
        let jitter = random % ceiling.saturating_add(1);
        match retry_after_ms {
            Some(ra) if ra > self.max_retry_after_ms => None,
            Some(ra) => Some(jitter.max(ra)),
            None => Some(jitter),
        }
    }
}

/// A failed XRPC (or plain HTTP) call.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum XrpcCallError {
    /// HTTP 429 or `RateLimitExceeded`.
    #[error("rate limited{}", .retry_after_ms.map(|ms| format!(" (retry after {ms} ms)")).unwrap_or_default())]
    RateLimited {
        /// From `Retry-After` (seconds or HTTP date) or `RateLimit-Reset`.
        retry_after_ms: Option<u64>,
    },
    /// `InvalidSwap`: a `swapCommit`/`swapRecord` precondition failed.
    #[error("InvalidSwap{}", fmt_message(.message))]
    InvalidSwap {
        /// The server's message.
        message: Option<String>,
    },
    /// `RecordNotFound`.
    #[error("RecordNotFound")]
    RecordNotFound,
    /// `RepoNotFound`.
    #[error("RepoNotFound")]
    RepoNotFound,
    /// `RepoDeactivated`.
    #[error("RepoDeactivated")]
    RepoDeactivated,
    /// `RepoTakendown`.
    #[error("RepoTakendown")]
    RepoTakendown,
    /// `RepoSuspended`.
    #[error("RepoSuspended")]
    RepoSuspended,
    /// HTTP 401, an authentication error name, or no usable session in the host.
    #[error("authentication required{}", fmt_message(.message))]
    AuthRequired {
        /// The XRPC error name, if any (`AuthRequired`, `ExpiredToken`, `InvalidToken`).
        error: Option<String>,
        /// The server's or host's message.
        message: Option<String>,
    },
    /// Any other XRPC error body.
    #[error("{error} (HTTP {status}){}", fmt_message(.message))]
    Xrpc {
        /// HTTP status.
        status: u16,
        /// Error name.
        error: String,
        /// The server's message.
        message: Option<String>,
    },
    /// A non-2xx status without an XRPC error body (including an unfollowed 3xx).
    #[error("HTTP {0}")]
    Http(u16),
    /// Connection, DNS or TLS failure.
    #[error("network error: {0}")]
    Network(String),
    /// The request timed out.
    #[error("timed out")]
    Timeout,
    /// The response exceeded the call's byte cap.
    #[error("response too large")]
    TooLarge,
    /// Refused by the host's policy (SSRF guard, scheme, redirect limit).
    #[error("blocked by policy: {0}")]
    Blocked(String),
    /// A browser refused a cross-origin response.
    #[error("cross-origin request refused")]
    Cors,
    /// Not supported by this host.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A 2xx response whose body does not decode.
    #[error("invalid response: {0}")]
    Decode(String),
    /// The call could not be built.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Any other host failure.
    #[error("{0}")]
    Other(String),
}

fn fmt_message(m: &Option<String>) -> String {
    m.as_deref().map(|m| format!(": {m}")).unwrap_or_default()
}

/// Truncates untrusted server text to `max` bytes on a char boundary.
fn bounded(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

const MAX_ERROR_NAME: usize = 128;
const MAX_ERROR_MESSAGE: usize = 512;

impl XrpcCallError {
    /// Maps a non-2xx response. `now_ms` is used to interpret an HTTP-date `Retry-After`.
    pub fn from_response(resp: &HttpResponse, now_ms: i64) -> Self {
        let body: Option<XrpcError> = serde_json::from_slice(&resp.body).ok();
        let name = body.as_ref().map(|b| bounded(&b.error, MAX_ERROR_NAME));
        let message = body
            .as_ref()
            .and_then(|b| b.message.as_deref())
            .map(|m| bounded(m, MAX_ERROR_MESSAGE));
        Self::from_parts(resp.status, name, message, || retry_after_ms(resp, now_ms))
    }

    fn from_parts(
        status: u16,
        name: Option<String>,
        message: Option<String>,
        retry_after: impl FnOnce() -> Option<u64>,
    ) -> Self {
        match name.as_deref() {
            Some("RateLimitExceeded") => {
                return Self::RateLimited {
                    retry_after_ms: retry_after(),
                };
            }
            Some("InvalidSwap") => return Self::InvalidSwap { message },
            Some("RecordNotFound") => return Self::RecordNotFound,
            Some("RepoNotFound") => return Self::RepoNotFound,
            Some("RepoDeactivated") => return Self::RepoDeactivated,
            Some("RepoTakendown") => return Self::RepoTakendown,
            Some("RepoSuspended") => return Self::RepoSuspended,
            Some("AuthRequired" | "AuthenticationRequired" | "ExpiredToken" | "InvalidToken") => {
                return Self::AuthRequired {
                    error: name,
                    message,
                };
            }
            _ => {}
        }
        match (status, name) {
            (429, _) => Self::RateLimited {
                retry_after_ms: retry_after(),
            },
            (401, error) => Self::AuthRequired { error, message },
            (status, Some(error)) => Self::Xrpc {
                status,
                error,
                message,
            },
            (status, None) => Self::Http(status),
        }
    }

    /// Maps a host failure.
    pub fn from_host(e: HostError) -> Self {
        match e {
            HostError::Network(s) => Self::Network(s),
            HostError::Timeout => Self::Timeout,
            HostError::TooLarge => Self::TooLarge,
            HostError::Blocked(s) => Self::Blocked(s),
            HostError::Cors => Self::Cors,
            HostError::AuthRequired(s) => Self::AuthRequired {
                error: None,
                message: Some(s),
            },
            HostError::Unsupported(s) => Self::Unsupported(s),
            HostError::HttpStatus(status, name) => Self::from_parts(status, name, None, || None),
            other => Self::Other(other.to_string()),
        }
    }

    /// True for failures worth retrying: network errors, timeouts, 5xx and rate limits.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Network(_) | Self::Timeout | Self::RateLimited { .. } => true,
            Self::Http(s) | Self::Xrpc { status: s, .. } => *s >= 500,
            _ => false,
        }
    }

    /// The HTTP status, when the failure came from a response.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http(s) | Self::Xrpc { status: s, .. } => Some(*s),
            Self::RateLimited { .. } => Some(429),
            _ => None,
        }
    }

    /// The XRPC error name, when there was one.
    pub fn error_name(&self) -> Option<&str> {
        match self {
            Self::RateLimited { .. } => Some("RateLimitExceeded"),
            Self::InvalidSwap { .. } => Some("InvalidSwap"),
            Self::RecordNotFound => Some("RecordNotFound"),
            Self::RepoNotFound => Some("RepoNotFound"),
            Self::RepoDeactivated => Some("RepoDeactivated"),
            Self::RepoTakendown => Some("RepoTakendown"),
            Self::RepoSuspended => Some("RepoSuspended"),
            Self::AuthRequired { error, .. } => error.as_deref(),
            Self::Xrpc { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The server-requested wait, if any.
    pub fn retry_after_ms(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_ms } => *retry_after_ms,
            _ => None,
        }
    }

    /// True for `RepoNotFound`, `RepoDeactivated`, `RepoTakendown` and `RepoSuspended`: the
    /// identity must be refreshed (`sync.md` §3.3).
    pub fn is_repo_status(&self) -> bool {
        matches!(
            self,
            Self::RepoNotFound | Self::RepoDeactivated | Self::RepoTakendown | Self::RepoSuspended
        )
    }
}

/// The wait requested by a response: `Retry-After` (delta seconds or an HTTP date), else
/// `RateLimit-Reset` (`sync.md` §9.2: seconds from now, or epoch seconds when above 10^9).
pub fn retry_after_ms(resp: &HttpResponse, now_ms: i64) -> Option<u64> {
    if let Some(ms) = resp
        .header("retry-after")
        .and_then(|v| parse_retry_after(v, now_ms))
    {
        return Some(ms);
    }
    let t: u64 = resp.header("ratelimit-reset")?.trim().parse().ok()?;
    if t > 1_000_000_000 {
        let at_ms = i64::try_from(t.saturating_mul(1000)).ok()?;
        Some(u64::try_from(at_ms.saturating_sub(now_ms)).unwrap_or(0))
    } else {
        Some(t.saturating_mul(1000))
    }
}

/// Parses a `Retry-After` value: delta seconds, or an IMF-fixdate (`Sun, 06 Nov 1994 08:49:37
/// GMT`) relative to `now_ms`. A date in the past gives 0.
pub fn parse_retry_after(value: &str, now_ms: i64) -> Option<u64> {
    let v = value.trim();
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        return v.parse::<u64>().ok().map(|s| s.saturating_mul(1000));
    }
    let at = parse_http_date(v)?;
    Some(u64::try_from(at.saturating_sub(now_ms)).unwrap_or(0))
}

/// Parses an IMF-fixdate (RFC 9110 §5.6.7) to milliseconds since the Unix epoch.
pub fn parse_http_date(s: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let parts: Vec<&str> = s.split_ascii_whitespace().collect();
    let [dow, day, mon, year, time, "GMT"] = parts.as_slice() else {
        return None;
    };
    if !DAYS.contains(&dow.strip_suffix(',')?) {
        return None;
    }
    let num = |t: &str, len: usize| -> Option<i64> {
        (t.len() == len && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok())?
    };
    let d = num(day, 2)?;
    let m = MONTHS.iter().position(|x| x == mon)? as i64 + 1;
    let y = num(year, 4)?;
    let mut hms = time.split(':');
    let (hh, mm, ss) = (
        num(hms.next()?, 2)?,
        num(hms.next()?, 2)?,
        num(hms.next()?, 2)?,
    );
    if hms.next().is_some() || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    Some(((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Decodes a JSON body; an empty body decodes as `null` (for `()` / `Option` outputs).
pub fn decode_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, XrpcCallError> {
    let body = if body.iter().all(u8::is_ascii_whitespace) {
        b"null".as_slice()
    } else {
        body
    };
    serde_json::from_slice(body).map_err(|e| XrpcCallError::Decode(e.to_string()))
}

fn random_u64<H: Host>(host: &H) -> u64 {
    let mut b = [0u8; 8];
    host.fill_random(&mut b);
    u64::from_le_bytes(b)
}

/// An XRPC client borrowing a [`Host`].
pub struct Xrpc<'h, H: Host> {
    host: &'h H,
    policy: RetryPolicy,
}

impl<'h, H: Host> Xrpc<'h, H> {
    /// A client with the default [`RetryPolicy`].
    pub fn new(host: &'h H) -> Self {
        Self {
            host,
            policy: RetryPolicy::default(),
        }
    }

    /// Replaces the retry policy.
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sends a request; a 2xx response is `Ok`, anything else an error. GETs are retried on
    /// transient failures when `retry` is set.
    pub async fn send(&self, req: HttpRequest, retry: bool) -> Result<HttpResponse, XrpcCallError> {
        let retry = retry && req.method == http::Method::GET;
        let mut attempt = 0u32;
        loop {
            let err = match self.host.http(req.clone()).await {
                Ok(resp) if (200..300).contains(&resp.status) => return Ok(resp),
                Ok(resp) => XrpcCallError::from_response(&resp, self.host.now_ms()),
                Err(e) => XrpcCallError::from_host(e),
            };
            if !retry || attempt >= self.policy.max_retries || !err.is_transient() {
                return Err(err);
            }
            let Some(delay) =
                self.policy
                    .delay_ms(attempt, random_u64(self.host), err.retry_after_ms())
            else {
                return Err(err);
            };
            tracing::debug!(attempt, delay, error = %err, "retrying GET");
            self.host.sleep(delay).await;
            attempt += 1;
        }
    }

    /// A GET of `path_and_query` at `service`.
    pub async fn get(
        &self,
        service: &Service,
        path_and_query: &str,
        opts: &CallOpts,
    ) -> Result<HttpResponse, XrpcCallError> {
        self.get_target(service.target(path_and_query)?, opts).await
    }

    /// An anonymous GET of an absolute URL (DoH, PLC, `.well-known` documents).
    pub async fn get_url(
        &self,
        url: url::Url,
        opts: &CallOpts,
    ) -> Result<HttpResponse, XrpcCallError> {
        self.get_target(Target::Anon(url), opts).await
    }

    async fn get_target(
        &self,
        target: Target,
        opts: &CallOpts,
    ) -> Result<HttpResponse, XrpcCallError> {
        let mut req = HttpRequest::get(target)
            .with_max_bytes(opts.max_bytes)
            .with_timeout_ms(opts.timeout_ms)
            .with_redirects(opts.redirects);
        if let Some(a) = opts.accept {
            req = req.with_header("accept", a);
        }
        self.send(req, opts.retry).await
    }

    /// A query by NSID with `(name, value)` parameters, decoding a JSON output.
    pub async fn query<T: DeserializeOwned>(
        &self,
        service: &Service,
        nsid: &str,
        params: &[(&str, &str)],
        opts: &CallOpts,
    ) -> Result<T, XrpcCallError> {
        let qs = lessdugin_atp::xrpc::query_string(params);
        let path = if qs.is_empty() {
            format!("/xrpc/{nsid}")
        } else {
            format!("/xrpc/{nsid}?{qs}")
        };
        decode_json(&self.get(service, &path, opts).await?.body)
    }

    /// A typed query, returning the raw response (for binary outputs such as CARs).
    pub async fn query_raw<P: Endpoint + QueryParams>(
        &self,
        service: &Service,
        params: &P,
        opts: &CallOpts,
    ) -> Result<HttpResponse, XrpcCallError> {
        if P::METHOD != Method::Query {
            return Err(XrpcCallError::InvalidRequest(format!(
                "{} is not a query",
                P::NSID
            )));
        }
        self.get(service, &query_path(params), opts).await
    }

    /// A typed query with a JSON output.
    pub async fn query_endpoint<P>(
        &self,
        service: &Service,
        params: &P,
        opts: &CallOpts,
    ) -> Result<P::Output, XrpcCallError>
    where
        P: Endpoint + QueryParams,
        P::Output: DeserializeOwned,
    {
        decode_json(&self.query_raw(service, params, opts).await?.body)
    }

    /// A procedure by NSID with a JSON input and output. Never retried.
    pub async fn procedure<I: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        service: &Service,
        nsid: &str,
        input: &I,
        opts: &CallOpts,
    ) -> Result<O, XrpcCallError> {
        let body = serde_json::to_vec(input)
            .map_err(|e| XrpcCallError::InvalidRequest(format!("input: {e}")))?;
        let req = HttpRequest::post_json(service.target(&format!("/xrpc/{nsid}"))?, body)
            .with_max_bytes(opts.max_bytes)
            .with_timeout_ms(opts.timeout_ms)
            .with_redirects(Redirects::None);
        decode_json(&self.send(req, false).await?.body)
    }

    /// A typed procedure with a JSON input and output. Never retried.
    pub async fn procedure_endpoint<P>(
        &self,
        service: &Service,
        input: &P,
        opts: &CallOpts,
    ) -> Result<P::Output, XrpcCallError>
    where
        P: Endpoint + Serialize,
        P::Output: DeserializeOwned,
    {
        if P::METHOD != Method::Procedure {
            return Err(XrpcCallError::InvalidRequest(format!(
                "{} is not a procedure",
                P::NSID
            )));
        }
        let nsid = procedure_path::<P>();
        let nsid = nsid.trim_start_matches("/xrpc/");
        self.procedure(service, nsid, input, opts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn http_dates() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777_000)
        );
        assert_eq!(
            parse_http_date("Fri, 16 Oct 2026 11:49:23 GMT"),
            Some(1_792_151_363_000)
        );
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 UTC"), None);
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("Sun, 6 Nov 1994 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Foo 1994 08:49:37 GMT"), None);
    }

    #[test]
    fn retry_after_forms() {
        assert_eq!(parse_retry_after("120", 0), Some(120_000));
        assert_eq!(parse_retry_after(" 0 ", 0), Some(0));
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:00:10 GMT", 4_000),
            Some(6_000)
        );
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:00:10 GMT", 20_000),
            Some(0)
        );
        assert_eq!(parse_retry_after("soon", 0), None);
        let now = 1_000_000_000_000;
        let r = resp(429, &[("ratelimit-reset", "30")], "");
        assert_eq!(retry_after_ms(&r, now), Some(30_000));
        let r = resp(429, &[("ratelimit-reset", "1000000060")], "");
        assert_eq!(retry_after_ms(&r, now), Some(60_000));
        let r = resp(429, &[("retry-after", "2"), ("ratelimit-reset", "30")], "");
        assert_eq!(retry_after_ms(&r, now), Some(2_000));
    }

    #[test]
    fn error_mapping() {
        let m = |status, body: &str| XrpcCallError::from_response(&resp(status, &[], body), 0);
        assert_eq!(
            m(400, r#"{"error":"RecordNotFound","message":"nope"}"#),
            XrpcCallError::RecordNotFound
        );
        assert_eq!(
            m(
                400,
                r#"{"error":"InvalidSwap","message":"Commit was at x"}"#
            ),
            XrpcCallError::InvalidSwap {
                message: Some("Commit was at x".into())
            }
        );
        assert_eq!(
            m(400, r#"{"error":"RepoNotFound"}"#),
            XrpcCallError::RepoNotFound
        );
        assert_eq!(
            m(400, r#"{"error":"RepoDeactivated"}"#),
            XrpcCallError::RepoDeactivated
        );
        assert_eq!(
            m(400, r#"{"error":"RepoTakendown"}"#),
            XrpcCallError::RepoTakendown
        );
        assert_eq!(
            m(400, r#"{"error":"RepoSuspended"}"#),
            XrpcCallError::RepoSuspended
        );
        assert!(matches!(
            m(400, r#"{"error":"ExpiredToken"}"#),
            XrpcCallError::AuthRequired { .. }
        ));
        assert_eq!(
            m(401, ""),
            XrpcCallError::AuthRequired {
                error: None,
                message: None
            }
        );
        assert_eq!(
            m(429, ""),
            XrpcCallError::RateLimited {
                retry_after_ms: None
            }
        );
        assert!(matches!(
            m(400, r#"{"error":"RateLimitExceeded"}"#),
            XrpcCallError::RateLimited { .. }
        ));
        assert_eq!(
            m(400, r#"{"error":"InvalidRequest","message":"bad"}"#),
            XrpcCallError::Xrpc {
                status: 400,
                error: "InvalidRequest".into(),
                message: Some("bad".into())
            }
        );
        assert_eq!(m(404, "<html>"), XrpcCallError::Http(404));
        assert_eq!(m(302, ""), XrpcCallError::Http(302));
        assert_eq!(
            m(502, r#"{"message":"no error field"}"#),
            XrpcCallError::Http(502)
        );
        assert!(m(502, "").is_transient());
        assert!(m(503, r#"{"error":"Unavailable"}"#).is_transient());
        assert!(!m(400, r#"{"error":"InvalidRequest"}"#).is_transient());
        assert!(m(400, r#"{"error":"RepoTakendown"}"#).is_repo_status());
        // untrusted text is bounded
        let long = format!(r#"{{"error":"X","message":"{}"}}"#, "é".repeat(1000));
        let XrpcCallError::Xrpc {
            message: Some(msg), ..
        } = m(400, &long)
        else {
            panic!()
        };
        assert!(msg.len() <= MAX_ERROR_MESSAGE + 3);
        // host errors
        assert_eq!(
            XrpcCallError::from_host(HostError::Timeout),
            XrpcCallError::Timeout
        );
        assert!(XrpcCallError::from_host(HostError::Network("x".into())).is_transient());
        assert_eq!(
            XrpcCallError::from_host(HostError::HttpStatus(400, Some("BlobNotFound".into()))),
            XrpcCallError::Xrpc {
                status: 400,
                error: "BlobNotFound".into(),
                message: None
            }
        );
        assert!(!XrpcCallError::from_host(HostError::Cors).is_transient());
    }

    #[test]
    fn backoff() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_ms(0, 1000, None), Some(1000 % 251));
        assert!(p.delay_ms(1, u64::MAX, None).unwrap() <= 500);
        assert!(p.delay_ms(10, u64::MAX, None).unwrap() <= 8_000);
        assert!(p.delay_ms(60, u64::MAX, None).unwrap() <= 8_000);
        assert_eq!(p.delay_ms(0, 0, Some(5_000)), Some(5_000));
        assert_eq!(p.delay_ms(0, 0, Some(61_000)), None);
    }

    #[test]
    fn service_targets() {
        let s = Service::Anon(url::Url::parse("https://pds.example.com/").unwrap());
        assert_eq!(
            s.target("/xrpc/a.b.c?x=1").unwrap(),
            Target::Anon(url::Url::parse("https://pds.example.com/xrpc/a.b.c?x=1").unwrap())
        );
        let s = Service::Anon(url::Url::parse("https://example.com/api").unwrap());
        let Target::Anon(u) = s.target("/xrpc/a.b.c").unwrap() else {
            panic!()
        };
        assert_eq!(u.as_str(), "https://example.com/api/xrpc/a.b.c");
        let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
        assert_eq!(
            Service::UserPds(did.clone()).target("/xrpc/x.y.z").unwrap(),
            Target::UserPds {
                account: did,
                path_and_query: "/xrpc/x.y.z".into()
            }
        );
        assert_eq!(decode_json::<()>(b"").unwrap(), ());
        assert_eq!(decode_json::<Option<u8>>(b"  ").unwrap(), None);
        assert!(matches!(
            decode_json::<u8>(b"{"),
            Err(XrpcCallError::Decode(_))
        ));
    }
}
