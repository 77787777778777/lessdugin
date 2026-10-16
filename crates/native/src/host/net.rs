// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`Net`]: the only HTTP client of the native host. Every request passes the SSRF guard,
//! carries the lessdugin User-Agent, follows redirects only when asked (re-checking every hop),
//! and is capped in bytes (after decompression) and time.
//!
//! Auth sessions ([`super::AuthSession`]) send `UserPds` requests through
//! [`Net::execute`] too, built with [`NetRequest::for_user_pds`] (no redirects).

use std::sync::Arc;
use std::time::Duration;

use http::header::{HeaderName, HeaderValue, LOCATION};
use lessdugin_core::host::{
    HostError, HttpRequest, HttpResponse, Redirects, Target, is_sensitive_header,
};
use lessdugin_core::limits;
use url::Url;

use super::guard::{DnsBackend, GuardError, GuardedResolver, NetPolicy, SystemDns};

/// `User-Agent` of every native request (`threat-model.md` INFRA-10, `sync.md` §9.5).
pub const USER_AGENT: &str = concat!(
    "lessdugin/",
    env!("CARGO_PKG_VERSION"),
    " (+https://lessdugin.7778777.online)"
);

/// Request headers callers may not set: framing and routing are the client's business, and a
/// caller-set `accept-encoding` would disable transparent decompression (and so the
/// after-decompression byte count).
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "accept-encoding",
    "proxy-authorization",
    "proxy-connection",
];

/// One request for [`Net::execute`].
#[derive(Clone)]
pub struct NetRequest {
    /// Method.
    pub method: http::Method,
    /// Absolute URL.
    pub url: Url,
    /// Headers (lowercase names).
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Option<Vec<u8>>,
    /// Cap on the response body after decompression.
    pub max_bytes: u64,
    /// Total timeout (all hops and the body) in milliseconds.
    pub timeout_ms: u32,
    /// Redirects to follow (capped at [`limits::MAX_REDIRECTS`]); 0 returns a 3xx as is.
    pub max_redirects: u8,
}

impl std::fmt::Debug for NetRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if is_sensitive_header(k) {
                        "<redacted>"
                    } else {
                        v
                    },
                )
            })
            .collect();
        f.debug_struct("NetRequest")
            .field("method", &self.method)
            .field("url", &redact_url(&self.url))
            .field("headers", &headers)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .field("max_bytes", &self.max_bytes)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_redirects", &self.max_redirects)
            .finish()
    }
}

impl NetRequest {
    /// A GET of `url` with the default caps and no redirects.
    pub fn get(url: Url) -> Self {
        Self {
            method: http::Method::GET,
            url,
            headers: Vec::new(),
            body: None,
            max_bytes: limits::DEFAULT_RESPONSE_MAX_BYTES,
            timeout_ms: limits::REQUEST_TIMEOUT_MS,
            max_redirects: 0,
        }
    }

    /// The request for a [`Target::Anon`] [`HttpRequest`].
    pub fn from_anon(req: HttpRequest) -> Result<Self, HostError> {
        let Target::Anon(url) = req.target else {
            return Err(HostError::Other("not an anonymous request".into()));
        };
        Ok(Self {
            method: req.method,
            url,
            headers: req.headers,
            body: req.body,
            max_bytes: req.max_bytes,
            timeout_ms: req.timeout_ms,
            max_redirects: match req.redirects {
                Redirects::None => 0,
                Redirects::Follow(n) => n,
            },
        })
    }

    /// The request for a [`Target::UserPds`] [`HttpRequest`] to the PDS bound at login, `pds`
    /// (an origin such as `https://pds.example.com`). The path never changes the origin, and
    /// redirects are never followed (`threat-model.md` AUTH-8). The auth session adds its own
    /// `authorization`/`dpop` headers to the result.
    pub fn for_user_pds(pds: &Url, req: HttpRequest) -> Result<Self, HostError> {
        let Target::UserPds { path_and_query, .. } = &req.target else {
            return Err(HostError::Other("not a UserPds request".into()));
        };
        let url = user_pds_url(pds, path_and_query)?;
        Ok(Self {
            method: req.method,
            url,
            headers: req.headers,
            body: req.body,
            max_bytes: req.max_bytes,
            timeout_ms: req.timeout_ms,
            max_redirects: 0,
        })
    }
}

/// `pds` + `path_and_query`, refusing anything that could leave the PDS origin
/// (`//host`, backslashes, fragments, missing leading `/`).
pub fn user_pds_url(pds: &Url, path_and_query: &str) -> Result<Url, HostError> {
    if !path_and_query.starts_with('/')
        || path_and_query.starts_with("//")
        || path_and_query.contains(['\\', '#'])
        || path_and_query.chars().any(|c| c.is_control() || c == ' ')
    {
        return Err(HostError::Blocked(format!(
            "invalid PDS path {path_and_query:?}"
        )));
    }
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };
    let mut url = pds.clone();
    url.set_path(path);
    url.set_query(query);
    url.set_fragment(None);
    if url.origin() != pds.origin() {
        return Err(HostError::Blocked("PDS path changes the origin".into()));
    }
    Ok(url)
}

/// A URL for logs: scheme, host, port and path; no query (it can carry identifiers or codes).
pub fn redact_url(url: &Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.to_string()
}

/// The guarded HTTP client. Cloning shares the connection pool.
#[derive(Clone)]
pub struct Net {
    inner: Arc<NetInner>,
}

struct NetInner {
    client: reqwest::Client,
    policy: NetPolicy,
    resolver: GuardedResolver,
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Net")
            .field("policy", &self.inner.policy)
            .finish_non_exhaustive()
    }
}

impl Net {
    /// A client with the system resolver.
    pub fn new(policy: NetPolicy) -> Result<Self, HostError> {
        Self::with_dns(policy, Arc::new(SystemDns))
    }

    /// A client with another DNS backend (tests: [`super::guard::FixedDns`]).
    pub fn with_dns(policy: NetPolicy, dns: Arc<dyn DnsBackend>) -> Result<Self, HostError> {
        let resolver = GuardedResolver::new(policy.clone(), dns);
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            // Redirects are followed by `open`, which re-checks every hop.
            .redirect(reqwest::redirect::Policy::none())
            // A proxy would resolve and connect on our behalf, defeating the address pin.
            .no_proxy()
            .dns_resolver(Arc::new(resolver.clone()))
            .connect_timeout(Duration::from_millis(u64::from(limits::CONNECT_TIMEOUT_MS)))
            .referer(false)
            .build()
            .map_err(|e| HostError::Other(format!("cannot build the HTTP client: {e}")))?;
        Ok(Self {
            inner: Arc::new(NetInner {
                client,
                policy,
                resolver,
            }),
        })
    }

    /// The policy this client enforces.
    pub fn policy(&self) -> &NetPolicy {
        &self.inner.policy
    }

    /// The guard's resolver (for connections made outside `reqwest`, e.g. WebSockets).
    pub fn resolver(&self) -> &GuardedResolver {
        &self.inner.resolver
    }

    /// Sends an `http::Request` through the guard without following redirects: the entry
    /// point for adapters such as an `HttpClient` implementation for an OAuth library, so
    /// that its PAR, token, metadata and resource requests get the same SSRF guard, caps and
    /// User-Agent. Framing headers the client sets itself (`host`, `content-length`,
    /// `transfer-encoding`, `connection`, `accept-encoding`) are dropped rather than refused.
    pub async fn send_http(
        &self,
        req: http::Request<Vec<u8>>,
        max_bytes: u64,
        timeout_ms: u32,
    ) -> Result<http::Response<Vec<u8>>, HostError> {
        let (parts, body) = req.into_parts();
        let url = Url::parse(&parts.uri.to_string())
            .map_err(|_| HostError::Blocked(format!("not an absolute URL: {}", parts.uri)))?;
        let mut headers = Vec::with_capacity(parts.headers.len());
        for (k, v) in &parts.headers {
            if matches!(
                k.as_str(),
                "host" | "content-length" | "transfer-encoding" | "connection" | "accept-encoding"
            ) {
                continue;
            }
            let v = v
                .to_str()
                .map_err(|_| HostError::Other(format!("non-ASCII value for header {k}")))?;
            headers.push((k.as_str().to_owned(), v.to_owned()));
        }
        let resp = self
            .execute(NetRequest {
                method: parts.method,
                url,
                headers,
                body: (!body.is_empty()).then_some(body),
                max_bytes,
                timeout_ms,
                max_redirects: 0,
            })
            .await?;
        let mut out = http::Response::builder().status(resp.status);
        for (k, v) in &resp.headers {
            out = out.header(k.as_str(), v.as_str());
        }
        out.body(resp.body)
            .map_err(|e| HostError::Other(e.to_string()))
    }

    /// Performs `req`: guard, redirects, byte cap after decompression, total timeout.
    /// A non-2xx status is a successful [`HttpResponse`].
    pub async fn execute(&self, req: NetRequest) -> Result<HttpResponse, HostError> {
        let timeout = Duration::from_millis(u64::from(req.timeout_ms));
        let max = req.max_bytes;
        let work = async move {
            let resp = self.open(req).await?;
            read_capped(resp, max).await
        };
        tokio::time::timeout(timeout, work)
            .await
            .map_err(|_| HostError::Timeout)?
    }

    /// Sends `req` and follows redirects as allowed, returning the final response with its
    /// body unread. The caller applies its own timeout.
    pub(crate) async fn open(&self, req: NetRequest) -> Result<reqwest::Response, HostError> {
        let NetRequest {
            mut method,
            mut url,
            mut headers,
            mut body,
            max_redirects,
            ..
        } = req;
        let max_follow = max_redirects.min(limits::MAX_REDIRECTS);
        let mut header_map = build_headers(&headers)?;
        let mut hops = 0u8;
        loop {
            self.inner.policy.check_url(&url)?;
            let mut rb = self
                .inner
                .client
                .request(method.clone(), url.clone())
                .headers(header_map.clone());
            if let Some(b) = &body {
                rb = rb.body(b.clone());
            }
            tracing::debug!(method = %method, url = %redact_url(&url), hop = hops, "http request");
            let resp = rb.send().await.map_err(map_reqwest_error)?;
            let status = resp.status().as_u16();
            if max_follow == 0 || !matches!(status, 301 | 302 | 303 | 307 | 308) {
                tracing::debug!(status, url = %redact_url(&url), "http response");
                return Ok(resp);
            }
            if hops >= max_follow {
                return Err(HostError::Blocked(format!(
                    "more than {max_follow} redirects"
                )));
            }
            let next = resp
                .headers()
                .get(LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| url.join(l).ok())
                .ok_or_else(|| HostError::Blocked("redirect without a valid Location".into()))?;
            if url.scheme() == "https" && next.scheme() != "https" {
                return Err(HostError::Blocked(
                    "redirect from https to a non-https URL".into(),
                ));
            }
            if status == 303 || (matches!(status, 301 | 302) && method == http::Method::POST) {
                method = http::Method::GET;
                body = None;
                headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
            }
            if next.origin() != url.origin() {
                headers.retain(|(k, _)| !is_sensitive_header(k));
            }
            header_map = build_headers(&headers)?;
            tracing::debug!(status, from = %redact_url(&url), to = %redact_url(&next), "redirect");
            url = next;
            hops += 1;
        }
    }
}

fn build_headers(headers: &[(String, String)]) -> Result<http::HeaderMap, HostError> {
    let mut map = http::HeaderMap::new();
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| HostError::Other(format!("invalid header name {k:?}")))?;
        if FORBIDDEN_HEADERS.contains(&name.as_str()) {
            return Err(HostError::Blocked(format!("header {name} may not be set")));
        }
        let value = HeaderValue::from_str(v)
            .map_err(|_| HostError::Other(format!("invalid value for header {name}")))?;
        map.append(name, value);
    }
    Ok(map)
}

/// Response headers as lowercase (name, value) pairs; non-UTF-8 values are converted lossily.
pub(crate) fn collect_headers(map: &http::HeaderMap) -> Vec<(String, String)> {
    map.iter()
        .map(|(k, v)| {
            (
                k.as_str().to_owned(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// Reads the body, counting decompressed bytes and failing at the first chunk over `max`.
/// `Content-Length` is never trusted.
pub(crate) async fn read_capped(
    mut resp: reqwest::Response,
    max: u64,
) -> Result<HttpResponse, HostError> {
    let status = resp.status().as_u16();
    let headers = collect_headers(resp.headers());
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(map_reqwest_error)? {
        if body.len() as u64 + chunk.len() as u64 > max {
            return Err(HostError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// Maps a `reqwest` error: the guard's own errors (inside the connector's error chain) become
/// `Blocked`, timeouts `Timeout`, everything else `Network`. URLs are not included.
pub(crate) fn map_reqwest_error(e: reqwest::Error) -> HostError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(s) = source {
        if let Some(g) = s.downcast_ref::<GuardError>() {
            return g.clone().into();
        }
        source = s.source();
    }
    if e.is_timeout() {
        return HostError::Timeout;
    }
    let e = e.without_url();
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    HostError::Network(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_pds_urls_stay_on_origin() {
        let pds = Url::parse("https://pds.example.com").unwrap();
        let u = user_pds_url(&pds, "/xrpc/com.atproto.repo.putRecord?x=1").unwrap();
        assert_eq!(
            u.as_str(),
            "https://pds.example.com/xrpc/com.atproto.repo.putRecord?x=1"
        );
        let u = user_pds_url(&pds, "/xrpc/../../etc").unwrap();
        assert_eq!(u.origin(), pds.origin());
        for bad in [
            "//evil.com/x",
            "xrpc/x",
            "/x#frag",
            "/\\evil.com",
            "/x y",
            "",
            "/x\n",
        ] {
            assert!(user_pds_url(&pds, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn forbidden_headers_are_refused() {
        assert!(build_headers(&[("accept".into(), "application/json".into())]).is_ok());
        for h in [
            "host",
            "Accept-Encoding",
            "content-length",
            "transfer-encoding",
        ] {
            assert!(matches!(
                build_headers(&[(h.into(), "x".into())]),
                Err(HostError::Blocked(_))
            ));
        }
        assert!(build_headers(&[("bad name".into(), "x".into())]).is_err());
        assert!(build_headers(&[("x".into(), "a\nb".into())]).is_err());
    }

    #[test]
    fn debug_redacts() {
        let mut r = NetRequest::get(Url::parse("https://h.example/p?code=secret").unwrap());
        r.headers.push(("authorization".into(), "DPoP tok".into()));
        r.body = Some(b"password".to_vec());
        let s = format!("{r:?}");
        assert!(
            !s.contains("secret") && !s.contains("tok") && !s.contains("password"),
            "{s}"
        );
        assert!(
            USER_AGENT.starts_with("lessdugin/")
                && USER_AGENT.ends_with("(+https://lessdugin.7778777.online)")
        );
    }
}
