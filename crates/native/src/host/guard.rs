// SPDX-License-Identifier: AGPL-3.0-or-later
//! The SSRF guard (`threat-model.md` §5.6.2, INFRA-8, INFRA-9; `sync.md` §3.7, §12.1).
//!
//! Two layers, both applied to every hop of every request:
//! 1. [`NetPolicy::check_url`] before a connection: scheme (`https`/`wss`; `http`/`ws` only for
//!    configured devnet origins), no userinfo, no IP-literal hosts (devnet: loopback literals),
//!    a usable port.
//! 2. [`GuardedResolver`], the resolver the HTTP client and the WebSocket connector actually
//!    connect with: it resolves A and AAAA once, rejects the name if **any** answer is in
//!    [`is_blocked`] (devnet: loopback allowed), and hands exactly the checked addresses to the
//!    connector. There is no second resolution, so DNS rebinding cannot swap the address
//!    between check and connect.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use futures::future::BoxFuture;
use lessdugin_core::host::HostError;
use url::{Host, Url};

/// Is `ip` outside the globally reachable unicast space? This is the `BLOCKED` table of
/// `threat-model.md` §5.6.2 (the IANA special-purpose registries' "not globally reachable"
/// entries, plus every IPv6 form that embeds an IPv4 address). Loopback is included; devnet
/// policies allow it separately.
pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn in_v4(ip: Ipv4Addr, net: [u8; 4], prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (u32::from(ip) & mask) == (u32::from(Ipv4Addr::from(net)) & mask)
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    const BLOCKED_V4: &[([u8; 4], u32)] = &[
        ([0, 0, 0, 0], 8),       // "this network", unspecified
        ([10, 0, 0, 0], 8),      // private
        ([100, 64, 0, 0], 10),   // CGNAT shared address space
        ([127, 0, 0, 0], 8),     // loopback
        ([169, 254, 0, 0], 16),  // link-local (cloud metadata)
        ([172, 16, 0, 0], 12),   // private
        ([192, 0, 0, 0], 24),    // IETF protocol assignments
        ([192, 0, 2, 0], 24),    // TEST-NET-1
        ([192, 88, 99, 0], 24),  // 6to4 relay anycast (deprecated)
        ([192, 168, 0, 0], 16),  // private
        ([198, 18, 0, 0], 15),   // benchmarking
        ([198, 51, 100, 0], 24), // TEST-NET-2
        ([203, 0, 113, 0], 24),  // TEST-NET-3
        ([224, 0, 0, 0], 4),     // multicast
        ([240, 0, 0, 0], 4),     // reserved, broadcast
    ];
    BLOCKED_V4
        .iter()
        .any(|&(net, prefix)| in_v4(ip, net, prefix))
}

fn in_v6(ip: Ipv6Addr, net: Ipv6Addr, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    (u128::from(ip) & mask) == (u128::from(net) & mask)
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    // On DNS64/NAT64 networks every IPv4-only host resolves into 64:ff9b::/96 (RFC 6052 §2.1),
    // with the IPv4 address in the last 32 bits; the translator connects to that IPv4 address,
    // so the IPv4 rules apply to it.
    if in_v6(ip, Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96) {
        let [.., a, b, c, d] = ip.octets();
        return is_blocked_v4(Ipv4Addr::new(a, b, c, d));
    }
    const fn v6(s: [u16; 8]) -> Ipv6Addr {
        Ipv6Addr::new(s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    }
    const BLOCKED_V6: &[(Ipv6Addr, u32)] = &[
        // ::/96 covers ::/128 (unspecified), ::1/128 (loopback) and the deprecated
        // IPv4-compatible addresses (::a.b.c.d), which embed an IPv4 address.
        (v6([0, 0, 0, 0, 0, 0, 0, 0]), 96),
        (v6([0, 0, 0, 0, 0, 0xffff, 0, 0]), 96), // IPv4-mapped
        (v6([0, 0, 0, 0, 0xffff, 0, 0, 0]), 96), // IPv4-translated (SIIT)
        // 64:ff9b::/96 (NAT64 well-known prefix) is decided by its embedded IPv4 address,
        // below. The local-use prefix leaves the embedding position to the operator, so the
        // embedded address cannot be checked: blocked.
        (v6([0x64, 0xff9b, 1, 0, 0, 0, 0, 0]), 48), // NAT64 local-use
        (v6([0x100, 0, 0, 0, 0, 0, 0, 0]), 64),     // discard-only
        (v6([0x2001, 0, 0, 0, 0, 0, 0, 0]), 23),    // IETF protocol assignments (incl. Teredo)
        (v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0]), 32), // documentation
        (v6([0x2002, 0, 0, 0, 0, 0, 0, 0]), 16),    // 6to4
        (v6([0x3fff, 0, 0, 0, 0, 0, 0, 0]), 20),    // documentation (RFC 9637)
        (v6([0x5f00, 0, 0, 0, 0, 0, 0, 0]), 16),    // segment routing SIDs (RFC 9602)
        (v6([0xfc00, 0, 0, 0, 0, 0, 0, 0]), 7),     // unique local
        (v6([0xfe80, 0, 0, 0, 0, 0, 0, 0]), 10),    // link-local
        (v6([0xfec0, 0, 0, 0, 0, 0, 0, 0]), 10),    // site-local (deprecated)
        (v6([0xff00, 0, 0, 0, 0, 0, 0, 0]), 8),     // multicast
    ];
    BLOCKED_V6
        .iter()
        .any(|&(net, prefix)| in_v6(ip, net, prefix))
}

/// Loopback in its canonical forms only (`127.0.0.0/8`, `::1`); IPv4-mapped loopback stays
/// blocked even in devnet mode.
pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[0] == 127,
        IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST,
    }
}

/// The network policy of a host: production rules, or devnet rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetPolicy {
    /// `Some` in devnet mode: the origins (lowercase host, port) that may use `http`/`ws`.
    devnet: Option<Vec<(String, u16)>>,
}

impl NetPolicy {
    /// Production: `https`/`wss` only, no IP literals, every `BLOCKED` address refused.
    pub fn production() -> Self {
        Self { devnet: None }
    }

    /// Devnet mode (`sync.md` §3.7, ADR 0006): loopback addresses (resolved or literal) are
    /// allowed for every host, and `http`/`ws` for exactly the origins of `endpoints` (e.g. the
    /// devnet PLC, PDS and Jetstream). Never enabled implicitly.
    pub fn devnet<'a>(endpoints: impl IntoIterator<Item = &'a Url>) -> Self {
        let origins = endpoints
            .into_iter()
            .filter_map(|u| Some((host_key(u)?, u.port_or_known_default()?)))
            .collect();
        Self {
            devnet: Some(origins),
        }
    }

    /// True in devnet mode.
    pub fn is_devnet(&self) -> bool {
        self.devnet.is_some()
    }

    fn insecure_allowed(&self, url: &Url) -> bool {
        let (Some(origins), Some(host), Some(port)) =
            (&self.devnet, host_key(url), url.port_or_known_default())
        else {
            return false;
        };
        origins.iter().any(|(h, p)| *h == host && *p == port)
    }

    /// Steps 1–3 of the guard for one URL (one hop). WebSocket URLs use `wss` (`ws`).
    pub fn check_url(&self, url: &Url) -> Result<(), HostError> {
        match url.scheme() {
            "https" | "wss" => {}
            "http" | "ws" if self.insecure_allowed(url) => {}
            "http" | "ws" => {
                return Err(HostError::Blocked(format!(
                    "plain {} is not allowed (TLS required) for {}",
                    url.scheme(),
                    url.host_str().unwrap_or("?")
                )));
            }
            other => {
                return Err(HostError::Blocked(format!(
                    "scheme {other:?} is not allowed"
                )));
            }
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(HostError::Blocked(
                "credentials in the URL are not allowed".into(),
            ));
        }
        match url.host() {
            None => return Err(HostError::Blocked("the URL has no host".into())),
            Some(Host::Domain("")) => {
                return Err(HostError::Blocked("the URL has no host".into()));
            }
            Some(Host::Domain(_)) => {}
            Some(Host::Ipv4(ip)) => self.check_literal(IpAddr::V4(ip))?,
            Some(Host::Ipv6(ip)) => self.check_literal(IpAddr::V6(ip))?,
        }
        match url.port_or_known_default() {
            Some(p) if p != 0 => Ok(()),
            _ => Err(HostError::Blocked("the URL has no usable port".into())),
        }
    }

    fn check_literal(&self, ip: IpAddr) -> Result<(), HostError> {
        if self.is_devnet() && is_loopback(ip) {
            Ok(())
        } else {
            Err(HostError::Blocked(format!(
                "IP-literal hosts are not allowed ({ip})"
            )))
        }
    }

    /// Step 5 for one resolved address.
    pub fn ip_allowed(&self, ip: IpAddr) -> bool {
        (self.is_devnet() && is_loopback(ip)) || !is_blocked(ip)
    }
}

/// The lowercase host of a URL as a policy key (IPv6 in brackets, as `host_str` gives it).
fn host_key(url: &Url) -> Option<String> {
    url.host_str()
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
}

/// A DNS lookup backend (A and AAAA). The system resolver natively; tests substitute a fixed
/// table to simulate rebinding.
pub trait DnsBackend: Send + Sync + 'static {
    /// Every address of `host`.
    fn lookup(&self, host: String) -> BoxFuture<'static, io::Result<Vec<IpAddr>>>;
}

/// The operating system resolver (`getaddrinfo` on a blocking thread).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDns;

impl DnsBackend for SystemDns {
    fn lookup(&self, host: String) -> BoxFuture<'static, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0)).await?;
            Ok(addrs.map(|a| a.ip()).collect())
        })
    }
}

/// A fixed DNS table (tests, offline fixtures). Each name maps to a list of answers; the
/// first lookup gets the first answer, later lookups the next ones, and the last answer
/// repeats. Unknown names fail like NXDOMAIN.
#[derive(Debug, Default)]
pub struct FixedDns {
    table: std::sync::Mutex<DnsTable>,
}

/// Name → (lookups so far, answers in order).
type DnsTable = std::collections::HashMap<String, (usize, Vec<Vec<IpAddr>>)>;

impl FixedDns {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers for `host`, in lookup order.
    pub fn insert(&self, host: &str, answers: Vec<Vec<IpAddr>>) -> &Self {
        let mut t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        t.insert(host.to_ascii_lowercase(), (0, answers));
        self
    }

    /// How many times `host` was looked up.
    pub fn lookups(&self, host: &str) -> usize {
        let t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        t.get(&host.to_ascii_lowercase()).map_or(0, |(n, _)| *n)
    }
}

impl DnsBackend for FixedDns {
    fn lookup(&self, host: String) -> BoxFuture<'static, io::Result<Vec<IpAddr>>> {
        let answer = {
            let mut t = self.table.lock().unwrap_or_else(|e| e.into_inner());
            match t.get_mut(&host.to_ascii_lowercase()) {
                Some((n, answers)) if !answers.is_empty() => {
                    let i = (*n).min(answers.len() - 1);
                    *n += 1;
                    Ok(answers[i].clone())
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no such host {host}"),
                )),
            }
        };
        Box::pin(async move { answer })
    }
}

impl<T: DnsBackend> DnsBackend for Arc<T> {
    fn lookup(&self, host: String) -> BoxFuture<'static, io::Result<Vec<IpAddr>>> {
        (**self).lookup(host)
    }
}

/// Why the guard refused a name. Travels through the HTTP client's error chain so that it can
/// be reported as [`HostError::Blocked`] rather than a network error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// A resolved address is blocked by policy.
    Blocked(String),
    /// The name did not resolve.
    Dns(String),
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuardError::Blocked(m) => write!(f, "blocked by policy: {m}"),
            GuardError::Dns(m) => write!(f, "DNS: {m}"),
        }
    }
}

impl std::error::Error for GuardError {}

impl From<GuardError> for HostError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::Blocked(m) => HostError::Blocked(m),
            GuardError::Dns(m) => HostError::Network(format!("DNS: {m}")),
        }
    }
}

/// The resolver connections are made with: one lookup, every answer checked, exactly the
/// checked addresses returned (the DNS-rebinding pin).
#[derive(Clone)]
pub struct GuardedResolver {
    policy: NetPolicy,
    backend: Arc<dyn DnsBackend>,
}

impl fmt::Debug for GuardedResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardedResolver")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl GuardedResolver {
    /// A resolver enforcing `policy` over `backend`.
    pub fn new(policy: NetPolicy, backend: Arc<dyn DnsBackend>) -> Self {
        Self { policy, backend }
    }

    /// Resolves `host` once and checks every answer; any blocked answer rejects the name.
    pub async fn resolve_checked(&self, host: &str) -> Result<Vec<IpAddr>, GuardError> {
        let host = host.trim_end_matches('.');
        let addrs = self
            .backend
            .lookup(host.to_owned())
            .await
            .map_err(|e| GuardError::Dns(format!("{host}: {e}")))?;
        if addrs.is_empty() {
            return Err(GuardError::Dns(format!("{host}: no addresses")));
        }
        if let Some(bad) = addrs.iter().find(|ip| !self.policy.ip_allowed(**ip)) {
            return Err(GuardError::Blocked(format!(
                "{host} resolves to a blocked address ({bad})"
            )));
        }
        Ok(addrs)
    }
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let this = self.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = this.resolve_checked(&host).await?;
            // Port 0 is replaced by the URL's port by the connector.
            let addrs: reqwest::dns::Addrs =
                Box::new(addrs.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn every_blocked_range() {
        let blocked = [
            "0.0.0.0",
            "0.255.255.255",
            "10.0.0.1",
            "10.255.255.255",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "127.255.255.255",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.0.170",
            "192.0.2.10",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            // IPv6
            "::",
            "::1",
            "::127.0.0.1",
            "::8.8.8.8",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:8.8.8.8",
            "::ffff:0:8.8.8.8",
            "64:ff9b::a00:1",
            "64:ff9b::7f00:1",
            "64:ff9b::c0a8:101",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::",
            "64:ff9b:1::1",
            "64:ff9b:1:ffff::808:808",
            "100::1",
            "2001::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:1ff:ffff::1",
            "2001:db8::1",
            "2002:c000:0204::1",
            "2002:0a00:0001::1",
            "3fff::1",
            "5f00::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "ff02::1",
            "ff0e::1",
        ];
        for a in blocked {
            assert!(is_blocked(ip(a)), "{a} must be blocked");
        }
        let allowed = [
            "1.1.1.1",
            "8.8.8.8",
            "9.255.255.255",
            "11.0.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "126.255.255.255",
            "128.0.0.1",
            "169.253.255.255",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.88.98.1",
            "192.167.255.255",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "2001:200::1",
            "2a00:1450::1",
            "2620:4f:8000::1",
            "64:ff9a::1",
            // NAT64 well-known prefix embedding a global IPv4 address
            "64:ff9b::8.8.8.8",
            "64:ff9b::101:101",
        ];
        for a in allowed {
            assert!(!is_blocked(ip(a)), "{a} must be allowed");
        }
    }

    #[test]
    fn url_checks_production() {
        let p = NetPolicy::production();
        let ok = |u: &str| p.check_url(&Url::parse(u).unwrap());
        assert!(ok("https://pds.example.com/xrpc/x").is_ok());
        assert!(ok("wss://jetstream.example.com/subscribe").is_ok());
        assert!(
            ok("https://pds.example.com:8443/").is_ok(),
            "any port for non-forum endpoints"
        );
        for bad in [
            "http://pds.example.com/",
            "ws://jetstream.example.com/",
            "ftp://example.com/",
            "file:///etc/passwd",
            "https://user:pw@example.com/",
            "https://user@example.com/",
            "https://8.8.8.8/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://0x7f.0.0.1/",
            "https://2130706433/",
            "https://[::ffff:7f00:1]/",
            "https://example.com:0/",
        ] {
            assert!(
                matches!(ok(bad), Err(HostError::Blocked(_))),
                "{bad} must be blocked: {:?}",
                ok(bad)
            );
        }
    }

    #[test]
    fn url_checks_devnet() {
        let pds = Url::parse("http://localhost:2583").unwrap();
        let plc = Url::parse("http://127.0.0.1:2582").unwrap();
        let p = NetPolicy::devnet([&pds, &plc]);
        let ok = |u: &str| p.check_url(&Url::parse(u).unwrap());
        assert!(ok("http://localhost:2583/xrpc/x").is_ok());
        assert!(ok("http://LOCALHOST:2583/").is_ok());
        assert!(ok("http://127.0.0.1:2582/did:plc:x").is_ok());
        assert!(
            ok("https://127.0.0.1:9999/").is_ok(),
            "loopback literal over TLS"
        );
        assert!(
            ok("http://localhost:2584/").is_err(),
            "http only for configured origins"
        );
        assert!(ok("http://pds.example.com/").is_err());
        assert!(
            ok("https://10.0.0.1/").is_err(),
            "private literals stay blocked"
        );
        assert!(
            ok("https://[::ffff:127.0.0.1]/").is_err(),
            "mapped loopback stays blocked"
        );
        assert!(p.ip_allowed(ip("127.0.0.1")) && p.ip_allowed(ip("::1")));
        assert!(!p.ip_allowed(ip("192.168.0.1")) && !p.ip_allowed(ip("::ffff:127.0.0.1")));
        assert!(!NetPolicy::production().ip_allowed(ip("127.0.0.1")));
    }

    #[tokio::test]
    async fn resolver_rejects_any_blocked_answer() {
        let dns = Arc::new(FixedDns::new());
        dns.insert(
            "public.test",
            vec![vec![ip("93.184.216.34"), ip("2606:2800:220:1::1")]],
        );
        dns.insert(
            "mixed.test",
            vec![vec![ip("93.184.216.34"), ip("10.0.0.5")]],
        );
        dns.insert("meta.test", vec![vec![ip("169.254.169.254")]]);
        dns.insert("mapped.test", vec![vec![ip("::ffff:192.168.0.1")]]);
        dns.insert("loop.test", vec![vec![ip("127.0.0.1"), ip("::1")]]);
        let prod = GuardedResolver::new(NetPolicy::production(), dns.clone());
        assert_eq!(prod.resolve_checked("public.test").await.unwrap().len(), 2);
        for host in ["mixed.test", "meta.test", "mapped.test", "loop.test"] {
            let e = prod.resolve_checked(host).await.unwrap_err();
            assert!(matches!(e, GuardError::Blocked(_)), "{host}: {e:?}");
        }
        assert!(matches!(
            prod.resolve_checked("nxdomain.test").await,
            Err(GuardError::Dns(_))
        ));
        let dev = GuardedResolver::new(NetPolicy::devnet([]), dns.clone());
        assert_eq!(dev.resolve_checked("loop.test").await.unwrap().len(), 2);
        assert!(dev.resolve_checked("mixed.test").await.is_err());
    }

    #[tokio::test]
    async fn rebinding_answers_are_each_checked() {
        // First answer public, second private: every resolution is checked on its own, so the
        // private one is refused when it is looked up.
        let dns = Arc::new(FixedDns::new());
        dns.insert(
            "rebind.test",
            vec![vec![ip("93.184.216.34")], vec![ip("192.168.1.10")]],
        );
        let r = GuardedResolver::new(NetPolicy::production(), dns.clone());
        assert!(r.resolve_checked("rebind.test").await.is_ok());
        assert!(matches!(
            r.resolve_checked("rebind.test").await,
            Err(GuardError::Blocked(_))
        ));
        assert_eq!(dns.lookups("rebind.test"), 2);
    }
}
