// SPDX-License-Identifier: AGPL-3.0-or-later
//! Request scheduling (`sync.md` §9): concurrency limits, per-host read budgets, circuit
//! breakers, PLC pacing and request accounting, for every request a sync job sends.
//!
//! [`Net`] is shared by every job of an engine. A job talks to the network through a
//! [`NetHost`]: a [`Host`] that wraps the real one, carries the job's [`Priority`] and
//! [`Stats`], and routes every attempt of every request through [`Net::send`]. The XRPC client
//! (and so its GET retries, §9.3) and the identity resolver run unchanged on top of it.
//!
//! ```text
//! send(req, priority):
//!   request cap reached (--max-requests)          → Blocked
//!   breaker open (§9.4)                           → Blocked, no request
//!   budget below the priority's floor, or 429'd   → wait until the window resets (≤ 60 s),
//!                                                    else a local 429 with Retry-After
//!   PLC: wait for the pacing slot (§3.5)
//!   wait for a slot: global, per host, PLC, DoH (§9.1); waiters by (priority, arrival)
//!   send; count it; read RateLimit-* (§9.2); 5xx / network / timeout / too large → breaker
//! ```
//!
//! Nothing here spawns, reads a clock or sleeps by itself: time is [`Host::now_ms`], waiting is
//! [`Host::sleep`], and the limiter is a plain future woken by released permits, so it runs the
//! same on a Tokio runtime and in a browser worker.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use lessdugin_atp::syntax::Did;

use crate::host::{BlobHandle, Host, HostError, HttpRequest, HttpResponse, MaybeSend, Target};
use crate::limits;
use crate::store::repo::{Breaker, BreakerState, Budget};

/// The internal response header carrying the send time of the attempt that produced a response
/// (ms; the order keys of `sync.md` §5.6.1 need it, and `HttpResponse` has no such field). It
/// never goes over the network.
pub const SENT_AT_HEADER: &str = "x-lessdugin-sent-at";
/// The internal response header marking a response made up locally (an exhausted read budget):
/// it was not sent and is not counted.
pub const LOCAL_HEADER: &str = "x-lessdugin-local";

/// Consecutive failures that open a host's breaker (`limits.md` §7 `BREAKER_FAILURES`).
pub const BREAKER_FAILURES: u32 = 10;
/// First open period of a breaker (`BREAKER_PROBE_MS`).
pub const BREAKER_PROBE_MS: i64 = 60_000;
/// Cap of the doubling open period (`BREAKER_PROBE_MAX_MS`).
pub const BREAKER_PROBE_MAX_MS: i64 = 3_600_000;
/// Static read budget of Bluesky hosts that send no `RateLimit-*` headers (ADR 0004).
pub const RATE_BUDGET_DEFAULT_REQUESTS: i64 = 3_000;
/// Window of that budget (ms).
pub const RATE_BUDGET_DEFAULT_WINDOW_MS: i64 = 300_000;
/// Starting (and maximum) PLC request rate per second (`sync.md` §3.5).
pub const PLC_RATE_MAX: u32 = 20;
/// The PLC rate grows by 2/s after this long without a 429.
pub const PLC_RECOVER_MS: i64 = 60_000;

/// Scheduling priority (`sync.md` §6.1). `P0` is the highest; the derived order sorts it first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Content on screen; outbox writes.
    P0,
    /// Posts, comments, declarations, modActions, memberships, revisions, sequences, wiki.
    #[default]
    P1,
    /// Opinions.
    P2,
    /// Reconcile.
    P3,
}

impl Priority {
    /// `"P0"` … `"P3"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
        }
    }
}

/// Concurrency limits (`sync.md` §9.1, *tunable*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetLimits {
    /// Requests in flight in total.
    pub global: usize,
    /// Requests in flight per host origin.
    pub per_host: usize,
    /// Requests in flight to the PLC directory.
    pub plc: usize,
    /// Requests in flight to DoH providers.
    pub doh: usize,
}

impl NetLimits {
    /// Native limits: 32 global, 6 per host, 8 PLC, 8 DoH.
    pub const NATIVE: Self = Self {
        global: limits::INFLIGHT_NATIVE,
        per_host: limits::INFLIGHT_PER_HOST,
        plc: limits::INFLIGHT_PLC,
        doh: limits::INFLIGHT_DOH,
    };
    /// Browser limits: 16 global, otherwise as native.
    pub const BROWSER: Self = Self {
        global: limits::INFLIGHT_BROWSER,
        ..Self::NATIVE
    };
}

/// The origin (`scheme://host[:port]`) budgets and breakers are kept by. `UserPds` targets are
/// keyed by account.
pub fn origin_of(target: &Target) -> String {
    match target {
        Target::Anon(u) => u.origin().ascii_serialization(),
        Target::UserPds { account, .. } => format!("userpds:{account}"),
    }
}

/// The origin of a URL.
pub fn url_origin(u: &url::Url) -> String {
    u.origin().ascii_serialization()
}

fn has_static_budget(origin: &str) -> bool {
    let host = origin
        .split_once("://")
        .map_or(origin, |(_, h)| h)
        .split(':')
        .next()
        .unwrap_or_default();
    host == "bsky.social" || host.ends_with(".bsky.network") || host == "bsky.network"
}

/// Which concurrency class a request falls in, beyond its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Plc,
    Doh,
    Other,
}

/// The `Blocked` message of a request refused by the request cap.
pub const REQUEST_LIMIT_REACHED: &str = "request limit reached";
/// The `Blocked` message of a request refused by the time budget.
pub const TIME_BUDGET_SPENT: &str = "time budget spent";

/// Requests sent and bytes received by one command (`SyncReport.requests`, `.bytes`), with an
/// optional cap on requests (`--max-requests`) and an optional deadline (`--time-budget`).
#[derive(Debug, Default)]
pub struct Stats {
    inner: Mutex<StatsInner>,
    max_requests: Option<u32>,
    /// No request is sent at or after this time (ms): the command's time budget.
    deadline: Option<i64>,
}

#[derive(Debug, Default)]
struct StatsInner {
    requests: u32,
    bytes: u64,
    per_origin: BTreeMap<String, u32>,
}

impl Stats {
    /// Counters without a cap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counters that refuse requests beyond `max`.
    pub fn capped(max: Option<u32>) -> Self {
        Self {
            inner: Mutex::new(StatsInner::default()),
            max_requests: max,
            deadline: None,
        }
    }

    /// Refuses requests at or after `deadline_ms` (a command's time budget, `--time-budget`):
    /// work in flight finishes, nothing new is sent, and the command reports `[partial]`.
    pub fn with_deadline(mut self, deadline_ms: Option<i64>) -> Self {
        self.deadline = deadline_ms;
        self
    }

    /// The deadline, if any.
    pub fn deadline(&self) -> Option<i64> {
        self.deadline
    }

    /// `true` once the time budget is spent at `now_ms`.
    pub fn expired(&self, now_ms: i64) -> bool {
        self.deadline.is_some_and(|d| now_ms >= d)
    }

    fn lock(&self) -> MutexGuard<'_, StatsInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Requests sent (every attempt).
    pub fn requests(&self) -> u32 {
        self.lock().requests
    }

    /// Response bytes received.
    pub fn bytes(&self) -> u64 {
        self.lock().bytes
    }

    /// Requests sent per origin.
    pub fn per_origin(&self) -> BTreeMap<String, u32> {
        self.lock().per_origin.clone()
    }

    /// `true` once the request cap is reached.
    pub fn exhausted(&self) -> bool {
        self.max_requests.is_some_and(|m| self.requests() >= m)
    }

    /// Reserves one request; `false` if the cap is reached.
    fn take(&self, origin: &str) -> bool {
        let mut s = self.lock();
        if self.max_requests.is_some_and(|m| s.requests >= m) {
            return false;
        }
        s.requests += 1;
        *s.per_origin.entry(origin.to_owned()).or_default() += 1;
        true
    }

    fn add_bytes(&self, n: usize) {
        self.lock().bytes += n as u64;
    }
}

/// Per-origin scheduling state.
#[derive(Clone, Debug, Default)]
struct HostState {
    inflight: usize,
    breaker: Breaker,
    probe_inflight: bool,
    budget: Option<Budget>,
    /// No dispatch before this time (a 429).
    blocked_until: i64,
    /// Seeded from a stored `HostRow` (or seen) already.
    known: bool,
}

impl HostState {
    fn refresh_budget(&mut self, now: i64) {
        if let Some(b) = &mut self.budget
            && now >= b.reset_at
        {
            b.est = b.limit;
            if b.reset_at <= now {
                b.reset_at = now.saturating_add(RATE_BUDGET_DEFAULT_WINDOW_MS);
            }
        }
    }

    /// When a request at `prio` may be dispatched (`None`: now).
    fn budget_wait(&mut self, origin: &str, prio: Priority, now: i64) -> Option<i64> {
        if self.blocked_until > now {
            return Some(self.blocked_until);
        }
        if self.budget.is_none() && has_static_budget(origin) {
            self.budget = Some(Budget {
                limit: RATE_BUDGET_DEFAULT_REQUESTS,
                est: RATE_BUDGET_DEFAULT_REQUESTS,
                reset_at: now.saturating_add(RATE_BUDGET_DEFAULT_WINDOW_MS),
            });
        }
        self.refresh_budget(now);
        let b = self.budget.as_ref()?;
        let floor = if prio == Priority::P0 {
            b.limit / 5
        } else {
            b.limit / 2
        };
        (b.est <= floor).then_some(b.reset_at.max(now + 1))
    }
}

/// The breaker gate's answer.
enum Gate {
    /// Send now (`probe`: as the half-open probe).
    Go { probe: bool },
    /// The breaker is open until this time.
    Open(i64),
    /// The budget allows it only at this time.
    Wait(i64),
}

/// PLC pacing (`sync.md` §3.5).
#[derive(Debug)]
struct PlcPacing {
    rate: u32,
    next_at: i64,
    last_429: i64,
    last_raise: i64,
}

impl Default for PlcPacing {
    fn default() -> Self {
        Self {
            rate: PLC_RATE_MAX,
            next_at: i64::MIN,
            last_429: i64::MIN,
            last_raise: i64::MIN,
        }
    }
}

impl PlcPacing {
    /// Takes the next slot at `now`; returns how long to wait for it.
    fn slot(&mut self, now: i64) -> i64 {
        // +2/s after every 60 s without a 429
        let since = now.saturating_sub(self.last_429.max(self.last_raise));
        if self.rate < PLC_RATE_MAX && since >= PLC_RECOVER_MS {
            let steps = (since / PLC_RECOVER_MS).min(10) as u32;
            self.rate = (self.rate + 2 * steps).min(PLC_RATE_MAX);
            self.last_raise = now;
        }
        let spacing = 1000 / i64::from(self.rate.max(1));
        let at = self.next_at.max(now);
        self.next_at = at.saturating_add(spacing);
        at - now
    }

    fn on_429(&mut self, now: i64) {
        self.rate = (self.rate / 2).max(1);
        self.last_429 = now;
    }
}

struct Waiter {
    origin: String,
    class: Class,
    waker: Option<Waker>,
}

#[derive(Default)]
struct State {
    global: usize,
    plc: usize,
    doh: usize,
    hosts: BTreeMap<String, HostState>,
    waiters: BTreeMap<(Priority, u64), Waiter>,
    seq: u64,
    plc_pacing: PlcPacing,
    locked: BTreeSet<String>,
    lock_waiters: BTreeMap<String, BTreeMap<u64, Option<Waker>>>,
}

impl State {
    fn host(&mut self, origin: &str) -> &mut HostState {
        self.hosts.entry(origin.to_owned()).or_default()
    }

    fn class_count(&self, class: Class) -> usize {
        match class {
            Class::Plc => self.plc,
            Class::Doh => self.doh,
            Class::Other => 0,
        }
    }

    /// `true` if the waiter `key` gets a slot now: walking the waiters by (priority, arrival)
    /// and tentatively granting every earlier waiter that fits, it still fits.
    fn can_grant(&self, key: (Priority, u64), lim: &NetLimits) -> bool {
        let mut global = self.global;
        let mut class_used: BTreeMap<Class, usize> = BTreeMap::new();
        let mut host_used: BTreeMap<&str, usize> = BTreeMap::new();
        for (k, w) in &self.waiters {
            let h = host_used
                .get(w.origin.as_str())
                .copied()
                .unwrap_or_else(|| self.hosts.get(&w.origin).map_or(0, |h| h.inflight));
            let c = class_used
                .get(&w.class)
                .copied()
                .unwrap_or_else(|| self.class_count(w.class));
            let class_cap = match w.class {
                Class::Plc => lim.plc,
                Class::Doh => lim.doh,
                Class::Other => usize::MAX,
            };
            let fits = global < lim.global && h < lim.per_host && c < class_cap;
            if *k == key {
                return fits;
            }
            if fits {
                global += 1;
                host_used.insert(&w.origin, h + 1);
                class_used.insert(w.class, c + 1);
            }
        }
        false
    }

    fn take(&mut self, origin: &str, class: Class) {
        self.global += 1;
        match class {
            Class::Plc => self.plc += 1,
            Class::Doh => self.doh += 1,
            Class::Other => {}
        }
        self.host(origin).inflight += 1;
    }

    fn release(&mut self, origin: &str, class: Class) {
        self.global = self.global.saturating_sub(1);
        match class {
            Class::Plc => self.plc = self.plc.saturating_sub(1),
            Class::Doh => self.doh = self.doh.saturating_sub(1),
            Class::Other => {}
        }
        let h = self.host(origin);
        h.inflight = h.inflight.saturating_sub(1);
    }

    fn wake_all(&mut self) {
        for w in self.waiters.values_mut() {
            if let Some(wk) = w.waker.take() {
                wk.wake();
            }
        }
    }
}

/// The shared scheduler: concurrency limits, budgets, breakers, PLC pacing and the per-repo
/// locks (`sync.md` §9, §10.6).
pub struct Net {
    limits: NetLimits,
    plc_origin: Option<String>,
    doh_origins: BTreeSet<String>,
    state: Mutex<State>,
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Net")
            .field("limits", &self.limits)
            .field("plc_origin", &self.plc_origin)
            .finish_non_exhaustive()
    }
}

impl Net {
    /// A scheduler with these limits; requests to `plc` and to the `doh` endpoints get their own
    /// caps (and PLC its pacing).
    pub fn new(limits: NetLimits, plc: Option<&url::Url>, doh: &[url::Url]) -> Self {
        Self {
            limits,
            plc_origin: plc.map(url_origin),
            doh_origins: doh.iter().map(url_origin).collect(),
            state: Mutex::new(State::default()),
        }
    }

    /// The limits.
    pub fn limits(&self) -> NetLimits {
        self.limits
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn class_of(&self, origin: &str) -> Class {
        if self.plc_origin.as_deref() == Some(origin) {
            Class::Plc
        } else if self.doh_origins.contains(origin) {
            Class::Doh
        } else {
            Class::Other
        }
    }

    /// Requests in flight now (all hosts).
    pub fn inflight(&self) -> usize {
        self.lock().global
    }

    // -----------------------------------------------------------------------------------------
    // Breakers and budgets

    /// Seeds an origin's breaker and budget from its stored `HostRow`, once per origin (a later
    /// observation in this process wins).
    pub fn seed_host(&self, origin: &str, breaker: &Breaker, budget: Option<&Budget>) {
        let mut st = self.lock();
        let h = st.host(origin);
        if !h.known {
            h.known = true;
            h.breaker = breaker.clone();
            if h.breaker.state == BreakerState::HalfOpen {
                // a probe that was in flight in another process: probe again
                h.breaker.state = BreakerState::Open;
            }
            h.budget = budget.cloned();
        }
    }

    /// The breaker and budget of an origin, to persist in its `HostRow`.
    pub fn host_snapshot(&self, origin: &str) -> (Breaker, Option<Budget>) {
        let mut st = self.lock();
        let h = st.host(origin);
        h.known = true;
        (h.breaker.clone(), h.budget.clone())
    }

    /// The breaker state of an origin.
    pub fn breaker(&self, origin: &str) -> Breaker {
        self.lock()
            .hosts
            .get(origin)
            .map(|h| h.breaker.clone())
            .unwrap_or_default()
    }

    /// The budget of an origin.
    pub fn budget(&self, origin: &str) -> Option<Budget> {
        self.lock().hosts.get(origin).and_then(|h| h.budget.clone())
    }

    fn gate(&self, origin: &str, prio: Priority, now: i64) -> Gate {
        let mut st = self.lock();
        let h = st.host(origin);
        h.known = true;
        match h.breaker.state {
            BreakerState::Closed => {}
            BreakerState::Open => {
                if now < h.breaker.until {
                    return Gate::Open(h.breaker.until);
                }
                if h.probe_inflight {
                    return Gate::Open(now + 1);
                }
                h.breaker.state = BreakerState::HalfOpen;
            }
            BreakerState::HalfOpen => {
                if h.probe_inflight {
                    return Gate::Open(now + 1);
                }
            }
        }
        if let Some(at) = h.budget_wait(origin, prio, now) {
            if h.breaker.state == BreakerState::HalfOpen {
                h.breaker.state = BreakerState::Open;
            }
            return Gate::Wait(at);
        }
        let probe = h.breaker.state == BreakerState::HalfOpen;
        if probe {
            h.probe_inflight = true;
        }
        Gate::Go { probe }
    }

    fn record_outcome(&self, origin: &str, failure: Option<bool>, probe: bool, now: i64) {
        let mut st = self.lock();
        let h = st.host(origin);
        if probe {
            h.probe_inflight = false;
        }
        match failure {
            // a neutral outcome (429, CORS, policy): only a probe's verdict matters
            None if probe => {
                h.breaker.state = BreakerState::Open;
                h.breaker.until = now;
            }
            None => {}
            Some(false) => {
                h.breaker.failures = 0;
                if h.breaker.state != BreakerState::Closed {
                    h.breaker = Breaker::default();
                }
            }
            Some(true) => breaker_failure(&mut h.breaker, probe, now),
        }
    }

    /// Counts one failure toward an origin's breaker for a response that broke a listing rule or
    /// a CID check (`limits.md` R4).
    pub fn strike(&self, origin: &str, now: i64) {
        let mut st = self.lock();
        breaker_failure(&mut st.host(origin).breaker, false, now);
    }

    fn on_send(&self, origin: &str, now: i64) {
        let mut st = self.lock();
        let h = st.host(origin);
        h.refresh_budget(now);
        if let Some(b) = &mut h.budget {
            b.est -= 1;
        }
    }

    fn on_response(&self, origin: &str, resp: &HttpResponse, now: i64) {
        let mut st = self.lock();
        let class = self.class_of(origin);
        let h = st.host(origin);
        let num = |k: &str| resp.header(k).and_then(|v| v.trim().parse::<i64>().ok());
        if let (Some(limit), Some(remaining)) = (num("ratelimit-limit"), num("ratelimit-remaining"))
            && limit > 0
        {
            let reset_at = match num("ratelimit-reset") {
                Some(t) if t > 1_000_000_000 => t.saturating_mul(1000),
                Some(t) => now.saturating_add(t.saturating_mul(1000)),
                None => now.saturating_add(RATE_BUDGET_DEFAULT_WINDOW_MS),
            };
            h.budget = Some(Budget {
                limit,
                est: remaining,
                reset_at,
            });
        }
        if resp.status == 429 {
            // a wait of 0 (a past reset or date) still waits 1 s
            let wait = crate::xrpc::retry_after_ms(resp, now)
                .filter(|w| *w > 0)
                .unwrap_or(1_000);
            let until = now.saturating_add(i64::try_from(wait).unwrap_or(i64::MAX));
            let reset = h.budget.as_ref().map_or(0, |b| b.reset_at);
            if let Some(b) = &mut h.budget {
                b.est = 0;
            }
            h.blocked_until = h.blocked_until.max(until.max(reset));
            if class == Class::Plc {
                st.plc_pacing.on_429(now);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Sending

    /// Sends one request attempt for a job at `prio` (see the module docs).
    pub async fn send<H: Host>(
        &self,
        host: &H,
        req: HttpRequest,
        prio: Priority,
        stats: &Stats,
    ) -> Result<HttpResponse, HostError> {
        let origin = origin_of(&req.target);
        let class = self.class_of(&origin);
        let probe = loop {
            let now = host.now_ms();
            match self.gate(&origin, prio, now) {
                Gate::Go { probe } => break probe,
                Gate::Open(until) => {
                    return Err(HostError::Blocked(format!(
                        "circuit breaker open for {origin} (for {} ms)",
                        until.saturating_sub(now).max(0)
                    )));
                }
                Gate::Wait(at) => {
                    let wait = at.saturating_sub(now).max(1);
                    if stats.deadline().is_some_and(|d| at >= d) {
                        // the budget frees up only after the command's time budget
                        return Err(HostError::Blocked(TIME_BUDGET_SPENT.into()));
                    }
                    if wait as u64 > limits::RETRY_AFTER_MAX_WAIT_MS {
                        return Ok(local_429(wait));
                    }
                    host.sleep(wait as u64).await;
                }
            }
        };
        if class == Class::Plc {
            let now = host.now_ms();
            let wait = self.lock().plc_pacing.slot(now);
            if wait > 0 {
                if stats.expired(now.saturating_add(wait)) {
                    self.record_outcome(&origin, None, probe, now);
                    return Err(HostError::Blocked(TIME_BUDGET_SPENT.into()));
                }
                host.sleep(wait as u64).await;
            }
        }
        let _permit = Acquire::new(self, &origin, class, prio).await;
        if stats.expired(host.now_ms()) {
            self.record_outcome(&origin, None, probe, host.now_ms());
            return Err(HostError::Blocked(TIME_BUDGET_SPENT.into()));
        }
        if !stats.take(&origin) {
            self.record_outcome(&origin, None, probe, host.now_ms());
            return Err(HostError::Blocked(REQUEST_LIMIT_REACHED.into()));
        }
        let sent_at = host.now_ms();
        self.on_send(&origin, sent_at);
        let result = host.http(req).await;
        let now = host.now_ms();
        match result {
            Ok(mut resp) => {
                stats.add_bytes(resp.body.len());
                self.on_response(&origin, &resp, now);
                let failure = match resp.status {
                    429 => None,
                    s if s >= 500 => Some(true),
                    _ => Some(false),
                };
                self.record_outcome(&origin, failure, probe, now);
                // the internal headers are ours alone: a server's copy is dropped before the
                // send time is stamped (it would otherwise be the first match)
                resp.headers.retain(|(k, _)| !is_internal_header(k));
                resp.headers
                    .push((SENT_AT_HEADER.to_owned(), sent_at.to_string()));
                Ok(resp)
            }
            Err(e) => {
                let failure = match &e {
                    HostError::Network(_) | HostError::Timeout | HostError::TooLarge => Some(true),
                    _ => None,
                };
                self.record_outcome(&origin, failure, probe, now);
                Err(e)
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Repo locks (`sync.md` §10.6, in process)

    /// Takes the in-process lock of repo `did` (the read-diff-write of `sync_repo`, point fetches
    /// and admissions for `did` never interleave).
    pub fn repo_lock(&self, did: &Did) -> RepoLock<'_> {
        RepoLock {
            net: self,
            key: did.to_string(),
            ticket: None,
            held: false,
        }
    }
}

fn breaker_failure(b: &mut Breaker, probe: bool, now: i64) {
    if probe || b.state == BreakerState::HalfOpen {
        b.open_ms = if b.open_ms <= 0 {
            BREAKER_PROBE_MS
        } else {
            b.open_ms.saturating_mul(2).min(BREAKER_PROBE_MAX_MS)
        };
        b.state = BreakerState::Open;
        b.until = now.saturating_add(b.open_ms);
        return;
    }
    b.failures = b.failures.saturating_add(1);
    if b.state == BreakerState::Closed && b.failures >= BREAKER_FAILURES {
        b.state = BreakerState::Open;
        b.open_ms = BREAKER_PROBE_MS;
        b.until = now.saturating_add(b.open_ms);
    }
}

fn local_429(wait_ms: i64) -> HttpResponse {
    let secs = (wait_ms + 999) / 1000;
    HttpResponse {
        status: 429,
        headers: vec![
            ("retry-after".to_owned(), secs.to_string()),
            (LOCAL_HEADER.to_owned(), "budget".to_owned()),
        ],
        body: br#"{"error":"RateLimitExceeded","message":"read budget exhausted"}"#.to_vec(),
    }
}

/// A slot in the limiter; dropping it frees the slot and wakes the waiters.
struct Permit<'a> {
    net: &'a Net,
    origin: String,
    class: Class,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut st = self.net.lock();
        st.release(&self.origin, self.class);
        st.wake_all();
    }
}

/// Waits for a slot (§9.1).
struct Acquire<'a> {
    net: &'a Net,
    origin: String,
    class: Class,
    prio: Priority,
    key: Option<(Priority, u64)>,
    done: bool,
}

impl<'a> Acquire<'a> {
    fn new(net: &'a Net, origin: &str, class: Class, prio: Priority) -> Self {
        Self {
            net,
            origin: origin.to_owned(),
            class,
            prio,
            key: None,
            done: false,
        }
    }
}

impl<'a> Future for Acquire<'a> {
    type Output = Permit<'a>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Permit<'a>> {
        let this = &mut *self;
        let net = this.net;
        let mut st = net.lock();
        let key = match this.key {
            Some(k) => k,
            None => {
                st.seq += 1;
                let k = (this.prio, st.seq);
                st.waiters.insert(
                    k,
                    Waiter {
                        origin: this.origin.clone(),
                        class: this.class,
                        waker: None,
                    },
                );
                this.key = Some(k);
                k
            }
        };
        if st.can_grant(key, &net.limits) {
            st.waiters.remove(&key);
            st.take(&this.origin, this.class);
            this.done = true;
            return Poll::Ready(Permit {
                net,
                origin: this.origin.clone(),
                class: this.class,
            });
        }
        if let Some(w) = st.waiters.get_mut(&key) {
            w.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for Acquire<'_> {
    fn drop(&mut self) {
        if !self.done
            && let Some(k) = self.key
        {
            let mut st = self.net.lock();
            st.waiters.remove(&k);
            st.wake_all();
        }
    }
}

/// The in-process lock of one repo; resolves to a guard.
pub struct RepoLock<'a> {
    net: &'a Net,
    key: String,
    ticket: Option<u64>,
    held: bool,
}

/// Holds a repo lock until dropped.
pub struct RepoGuard<'a> {
    net: &'a Net,
    key: String,
}

impl<'a> Future for RepoLock<'a> {
    type Output = RepoGuard<'a>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<RepoGuard<'a>> {
        let this = &mut *self;
        let mut st = this.net.lock();
        let ticket = match this.ticket {
            Some(t) => t,
            None => {
                st.seq += 1;
                let t = st.seq;
                st.lock_waiters
                    .entry(this.key.clone())
                    .or_default()
                    .insert(t, None);
                this.ticket = Some(t);
                t
            }
        };
        let first = st
            .lock_waiters
            .get(&this.key)
            .and_then(|q| q.keys().next().copied());
        if !st.locked.contains(&this.key) && first == Some(ticket) {
            st.locked.insert(this.key.clone());
            if let Some(q) = st.lock_waiters.get_mut(&this.key) {
                q.remove(&ticket);
                if q.is_empty() {
                    st.lock_waiters.remove(&this.key);
                }
            }
            this.held = true;
            return Poll::Ready(RepoGuard {
                net: this.net,
                key: this.key.clone(),
            });
        }
        if let Some(q) = st.lock_waiters.get_mut(&this.key) {
            q.insert(ticket, Some(cx.waker().clone()));
        }
        Poll::Pending
    }
}

impl Drop for RepoLock<'_> {
    fn drop(&mut self) {
        if self.held {
            return;
        }
        if let Some(t) = self.ticket {
            let mut st = self.net.lock();
            if let Some(q) = st.lock_waiters.get_mut(&self.key) {
                q.remove(&t);
                if let Some(Some(w)) = q.values().next().cloned() {
                    w.wake();
                }
                if q.is_empty() {
                    st.lock_waiters.remove(&self.key);
                }
            }
        }
    }
}

impl Drop for RepoGuard<'_> {
    fn drop(&mut self) {
        let mut st = self.net.lock();
        st.locked.remove(&self.key);
        if let Some(q) = st.lock_waiters.get_mut(&self.key)
            && let Some(Some(w)) = q.values().next().cloned()
        {
            w.wake();
        }
    }
}

/// A [`Host`] that sends every request through a [`Net`] at one priority, counting it in a
/// [`Stats`]. Everything else is the wrapped host's.
pub struct NetHost<H: Host> {
    inner: Arc<H>,
    net: Arc<Net>,
    prio: Priority,
    stats: Arc<Stats>,
}

impl<H: Host> Clone for NetHost<H> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            net: Arc::clone(&self.net),
            prio: self.prio,
            stats: Arc::clone(&self.stats),
        }
    }
}

impl<H: Host> NetHost<H> {
    /// Wraps `inner`.
    pub fn new(inner: Arc<H>, net: Arc<Net>, prio: Priority, stats: Arc<Stats>) -> Self {
        Self {
            inner,
            net,
            prio,
            stats,
        }
    }

    /// The same host at another priority (same counters).
    pub fn at(&self, prio: Priority) -> Self {
        Self {
            prio,
            ..self.clone()
        }
    }

    /// The priority.
    pub fn priority(&self) -> Priority {
        self.prio
    }

    /// The wrapped host.
    pub fn inner(&self) -> &H {
        &self.inner
    }

    /// The wrapped host, shared.
    pub fn shared_inner(&self) -> Arc<H> {
        Arc::clone(&self.inner)
    }

    /// The scheduler.
    pub fn net(&self) -> &Net {
        &self.net
    }

    /// The counters.
    pub fn stats(&self) -> &Arc<Stats> {
        &self.stats
    }
}

impl<H: Host> Host for NetHost<H> {
    type Kv = H::Kv;
    type Ws = H::Ws;

    fn cache(&self) -> &Self::Kv {
        self.inner.cache()
    }

    fn state(&self) -> &Self::Kv {
        self.inner.state()
    }

    fn http(
        &self,
        req: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HostError>> + MaybeSend {
        self.net.send(&*self.inner, req, self.prio, &self.stats)
    }

    fn fetch_blob(
        &self,
        pds: url::Url,
        did: Did,
        cid: String,
        max_bytes: u64,
    ) -> impl Future<Output = Result<BlobHandle, HostError>> + MaybeSend {
        self.inner.fetch_blob(pds, did, cid, max_bytes)
    }

    fn websocket(
        &self,
        url: url::Url,
        max_message_bytes: u32,
    ) -> impl Future<Output = Result<Self::Ws, HostError>> + MaybeSend {
        self.inner.websocket(url, max_message_bytes)
    }

    fn resolve_txt_system(
        &self,
        name: String,
    ) -> impl Future<Output = Result<Option<Vec<String>>, HostError>> + MaybeSend {
        self.inner.resolve_txt_system(name)
    }

    /// Sleeps (a retry back-off, a `Retry-After`), but never past the command's time budget:
    /// once it is spent every request is refused anyway.
    fn sleep(&self, ms: u64) -> impl Future<Output = ()> + MaybeSend {
        let ms = match self.stats.deadline() {
            Some(d) => ms.min(u64::try_from(d.saturating_sub(self.inner.now_ms())).unwrap_or(0)),
            None => ms,
        };
        self.inner.sleep(ms)
    }

    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }

    fn fill_random(&self, buf: &mut [u8]) {
        self.inner.fill_random(buf);
    }
}

/// `true` for a header name of the `x-lessdugin-` family, which only this engine sets on
/// responses ([`SENT_AT_HEADER`], [`LOCAL_HEADER`]); the same names from the network are removed.
pub fn is_internal_header(name: &str) -> bool {
    name.len() >= INTERNAL_HEADER_PREFIX.len()
        && name.as_bytes()[..INTERNAL_HEADER_PREFIX.len()]
            .eq_ignore_ascii_case(INTERNAL_HEADER_PREFIX.as_bytes())
}

/// The prefix of the internal response headers.
pub const INTERNAL_HEADER_PREFIX: &str = "x-lessdugin-";

/// The send time a [`Net`] stamped on a response, if any.
pub fn sent_at(resp: &HttpResponse) -> Option<i64> {
    resp.header(SENT_AT_HEADER)
        .and_then(|v| v.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> State {
        State::default()
    }

    #[test]
    fn grants_follow_priority_and_caps() {
        let lim = NetLimits {
            global: 2,
            per_host: 1,
            plc: 1,
            doh: 1,
        };
        let mut s = st();
        for (p, o) in [
            (Priority::P2, "a"),
            (Priority::P0, "a"),
            (Priority::P1, "b"),
            (Priority::P0, "c"),
        ] {
            s.seq += 1;
            s.waiters.insert(
                (p, s.seq),
                Waiter {
                    origin: o.into(),
                    class: Class::Other,
                    waker: None,
                },
            );
        }
        // order: (P0,2,a) (P0,4,c) (P1,3,b) (P2,1,a); global cap 2 → a and c
        assert!(s.can_grant((Priority::P0, 2), &lim));
        assert!(s.can_grant((Priority::P0, 4), &lim));
        assert!(!s.can_grant((Priority::P1, 3), &lim));
        assert!(!s.can_grant((Priority::P2, 1), &lim));
        // a host at its cap does not block another host's waiter
        s.take("a", Class::Other);
        assert!(!s.can_grant((Priority::P0, 2), &lim));
        assert!(s.can_grant((Priority::P0, 4), &lim));
        assert!(!s.can_grant((Priority::P1, 3), &lim));
    }

    #[test]
    fn breaker_opens_probes_and_doubles() {
        let net = Net::new(NetLimits::NATIVE, None, &[]);
        let o = "https://pds.example.com";
        for _ in 0..BREAKER_FAILURES - 1 {
            net.record_outcome(o, Some(true), false, 0);
        }
        assert_eq!(net.breaker(o).state, BreakerState::Closed);
        net.record_outcome(o, Some(false), false, 0); // a success resets the count
        for _ in 0..BREAKER_FAILURES {
            net.record_outcome(o, Some(true), false, 0);
        }
        let b = net.breaker(o);
        assert_eq!((b.state, b.until), (BreakerState::Open, BREAKER_PROBE_MS));
        assert!(matches!(net.gate(o, Priority::P0, 10), Gate::Open(_)));
        // half-open: one probe, others refused
        assert!(matches!(
            net.gate(o, Priority::P1, BREAKER_PROBE_MS),
            Gate::Go { probe: true }
        ));
        assert!(matches!(
            net.gate(o, Priority::P1, BREAKER_PROBE_MS),
            Gate::Open(_)
        ));
        // a failed probe doubles the open time
        net.record_outcome(o, Some(true), true, BREAKER_PROBE_MS);
        let b = net.breaker(o);
        assert_eq!(b.state, BreakerState::Open);
        assert_eq!(b.until, 3 * BREAKER_PROBE_MS);
        // … up to an hour
        let mut now = b.until;
        for _ in 0..20 {
            assert!(matches!(
                net.gate(o, Priority::P1, now),
                Gate::Go { probe: true }
            ));
            net.record_outcome(o, Some(true), true, now);
            now = net.breaker(o).until;
        }
        assert_eq!(net.breaker(o).open_ms, BREAKER_PROBE_MAX_MS);
        // a successful probe closes it
        assert!(matches!(
            net.gate(o, Priority::P1, now),
            Gate::Go { probe: true }
        ));
        net.record_outcome(o, Some(false), true, now);
        assert_eq!(net.breaker(o), Breaker::default());
    }

    #[test]
    fn budgets_reserve_headroom_for_p0() {
        let net = Net::new(NetLimits::NATIVE, None, &[]);
        let o = "https://x.host.bsky.network";
        // the static default applies to Bluesky hosts
        assert!(matches!(net.gate(o, Priority::P1, 0), Gate::Go { .. }));
        assert_eq!(net.budget(o).unwrap().limit, RATE_BUDGET_DEFAULT_REQUESTS);
        let resp = HttpResponse {
            status: 200,
            headers: vec![
                ("ratelimit-limit".into(), "100".into()),
                ("ratelimit-remaining".into(), "50".into()),
                ("ratelimit-reset".into(), "30".into()),
            ],
            body: Vec::new(),
        };
        net.on_response(o, &resp, 1_000);
        // 50 ≤ 100/2: background waits for the reset; P0 may still go (50 > 20)
        assert!(matches!(
            net.gate(o, Priority::P1, 1_000),
            Gate::Wait(31_000)
        ));
        assert!(matches!(net.gate(o, Priority::P0, 1_000), Gate::Go { .. }));
        assert!(matches!(net.gate(o, Priority::P1, 31_000), Gate::Go { .. }));
        // other hosts have no budget
        assert!(matches!(
            net.gate("https://pds.example.com", Priority::P3, 0),
            Gate::Go { .. }
        ));
        assert!(net.budget("https://pds.example.com").is_none());
        // a 429 blocks until Retry-After
        let r429 = HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "5".into())],
            body: Vec::new(),
        };
        net.on_response("https://pds.example.com", &r429, 0);
        assert!(matches!(
            net.gate("https://pds.example.com", Priority::P0, 10),
            Gate::Wait(5_000)
        ));
    }

    #[test]
    fn plc_pacing_halves_and_recovers() {
        let mut p = PlcPacing::default();
        assert_eq!(p.slot(0), 0);
        assert_eq!(p.slot(0), 50);
        assert_eq!(p.slot(0), 100);
        p.on_429(0);
        assert_eq!(p.rate, 10);
        p.next_at = 0;
        assert_eq!(p.slot(0), 0);
        assert_eq!(p.slot(0), 100);
        let _ = p.slot(PLC_RECOVER_MS);
        assert_eq!(p.rate, 12);
    }

    #[test]
    fn static_budget_hosts() {
        assert!(has_static_budget("https://bsky.social"));
        assert!(has_static_budget(
            "https://puffball.us-east.host.bsky.network"
        ));
        assert!(!has_static_budget("https://pds.example.com"));
        assert!(!has_static_budget("https://evilbsky.network.example"));
    }
}
