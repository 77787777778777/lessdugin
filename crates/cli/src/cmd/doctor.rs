// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin doctor [--pds <url>]`: configuration and paths, keyring, reachability of the
//! PLC directory, DoH, relays and Jetstream (short timeouts, no retries), the account's PDS
//! (`describeServer`, `oauth-protected-resource`), and clock skew against the PDS's `Date`.
//!
//! `--json` prints `{"dto": 1, "checks": [{"name", "status": "ok"|"warn"|"fail"|"skip",
//! "detail"}]}`. The exit code is 1 if any check failed.

use futures::future::join_all;
use lessdugin_core::dto::DTO_VERSION;
use lessdugin_core::host::{Host, HttpRequest, HttpResponse, Target};
use lessdugin_core::limits;
use lessdugin_core::xrpc::parse_http_date;
use lessdugin_native::auth::{KeyringStatus, StoredSession};
use lessdugin_native::config::NetworkProfile;
use serde::Serialize;
use url::Url;

use crate::args::DoctorArgs;
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, cat, lit, tt};

/// Timeout of each reachability probe.
const PROBE_TIMEOUT_MS: u32 = 5_000;
/// Clock skew that earns a warning.
const SKEW_WARN_MS: i64 = 60_000;

/// A check outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Fine.
    Ok,
    /// Works, with a caveat.
    Warn,
    /// Broken.
    Fail,
    /// Not applicable here.
    Skip,
}

/// One check.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    name: String,
    status: Status,
    detail: String,
}

fn check(name: impl Into<String>, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
    }
}

#[derive(Serialize)]
struct DoctorReport {
    dto: u32,
    checks: Vec<Check>,
}

/// Milliseconds since an earlier [`std::time::Instant`].
fn elapsed_ms(since: std::time::Instant) -> u128 {
    since.elapsed().as_millis()
}

#[allow(clippy::disallowed_methods)] // wall-clock latency of a probe, for display only
fn start() -> std::time::Instant {
    std::time::Instant::now()
}

async fn probe(ctx: &Ctx, url: &Url, accept: Option<&str>) -> Result<(HttpResponse, u128), String> {
    let engine = ctx.engine().map_err(|e| format!("{e:?}"))?;
    let mut req = HttpRequest::get(Target::Anon(url.clone()))
        .with_timeout_ms(PROBE_TIMEOUT_MS)
        .with_max_bytes(limits::SMALL_RESPONSE_MAX_BYTES);
    if let Some(a) = accept {
        req = req.with_header("accept", a);
    }
    let t = start();
    match engine.host().http(req).await {
        Ok(resp) => Ok((resp, elapsed_ms(t))),
        Err(e) => Err(e.to_string()),
    }
}

/// Any HTTP answer counts as reachable.
async fn reach(ctx: &Ctx, name: String, url: Url) -> Check {
    match probe(ctx, &url, None).await {
        Ok((resp, ms)) => check(
            name,
            Status::Ok,
            format!("{url} answered HTTP {} in {ms} ms", resp.status),
        ),
        Err(e) => check(name, Status::Fail, format!("{url}: {e}")),
    }
}

async fn doh(ctx: &Ctx, base: Url) -> Check {
    let mut url = base.clone();
    url.query_pairs_mut()
        .append_pair("name", "_atproto.bsky.app")
        .append_pair("type", "TXT");
    match probe(ctx, &url, Some("application/dns-json")).await {
        Ok((resp, ms)) if resp.status == 200 => check(
            format!("doh {}", host(&base)),
            Status::Ok,
            format!("answered in {ms} ms"),
        ),
        Ok((resp, _)) => check(
            format!("doh {}", host(&base)),
            Status::Fail,
            format!("HTTP {}", resp.status),
        ),
        Err(e) => check(format!("doh {}", host(&base)), Status::Fail, e),
    }
}

fn host(u: &Url) -> String {
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => u.to_string(),
    }
}

fn join(base: &Url, path: &str) -> Option<Url> {
    Url::parse(&format!("{}{path}", base.as_str().trim_end_matches('/'))).ok()
}

async fn pds_checks(ctx: &Ctx, pds: Url) -> Vec<Check> {
    let mut out = Vec::new();
    let label = format!("pds {}", host(&pds));
    let Some(describe) = join(&pds, "/xrpc/com.atproto.server.describeServer") else {
        return vec![check(label, Status::Fail, "not a valid URL")];
    };
    match probe(ctx, &describe, None).await {
        Ok((resp, ms)) if resp.status == 200 => {
            let did = serde_json::from_slice::<serde_json::Value>(&resp.body)
                .ok()
                .and_then(|v| v.get("did").and_then(|d| d.as_str()).map(str::to_owned))
                .unwrap_or_else(|| "?".into());
            out.push(check(
                format!("{label} describeServer"),
                Status::Ok,
                format!("server {did}, {ms} ms"),
            ));
            out.push(clock_skew(ctx, &resp));
        }
        Ok((resp, _)) => out.push(check(
            format!("{label} describeServer"),
            Status::Fail,
            format!("HTTP {}", resp.status),
        )),
        Err(e) => out.push(check(format!("{label} describeServer"), Status::Fail, e)),
    }
    if let Some(prm) = join(&pds, "/.well-known/oauth-protected-resource") {
        out.push(match probe(ctx, &prm, None).await {
            Ok((resp, _)) if resp.status == 200 => {
                let servers: Vec<String> = serde_json::from_slice::<serde_json::Value>(&resp.body)
                    .ok()
                    .and_then(|v| v.get("authorization_servers").cloned())
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                if servers.is_empty() {
                    check(
                        format!("{label} oauth"),
                        Status::Warn,
                        "no authorization server listed: use --app-password",
                    )
                } else {
                    check(
                        format!("{label} oauth"),
                        Status::Ok,
                        format!("authorization server {}", servers.join(", ")),
                    )
                }
            }
            Ok((resp, _)) => check(
                format!("{label} oauth"),
                Status::Warn,
                format!(
                    "no protected-resource metadata (HTTP {}): use --app-password",
                    resp.status
                ),
            ),
            Err(e) => check(format!("{label} oauth"), Status::Fail, e),
        });
    }
    out
}

fn clock_skew(ctx: &Ctx, resp: &HttpResponse) -> Check {
    let Some(server_ms) = resp.header("date").and_then(parse_http_date) else {
        return check("clock", Status::Skip, "the PDS sent no Date header");
    };
    let Ok(engine) = ctx.engine() else {
        return check("clock", Status::Skip, "no engine");
    };
    let skew = engine.host().now_ms() - server_ms;
    let abs = skew.abs();
    let detail = format!(
        "local clock is {} s {} the PDS",
        abs / 1000,
        if skew >= 0 { "ahead of" } else { "behind" }
    );
    let status = if abs > lessdugin_core::limits::FUTURE_SKEW_MS {
        Status::Fail
    } else if abs > SKEW_WARN_MS {
        Status::Warn
    } else {
        Status::Ok
    };
    check("clock", status, detail)
}

/// `lessdugin doctor`.
pub async fn run(ctx: &Ctx, args: DoctorArgs) -> Result<(), CliError> {
    let mut checks = Vec::new();
    let cfg = &ctx.config;
    let profile = match &cfg.network {
        NetworkProfile::Devnet(d) => format!("devnet (plc {}, pds {})", d.plc, d.pds),
        NetworkProfile::Production => "production".to_owned(),
    };
    checks.push(match &ctx.config_problem {
        None => check(
            "config",
            Status::Ok,
            format!("{} ({profile})", ctx.paths.config_file.display()),
        ),
        Some(p) => check("config", Status::Warn, format!("using defaults: {p}")),
    });
    checks.push(match ctx.paths.ensure_dirs() {
        Ok(()) => check(
            "paths",
            Status::Ok,
            format!(
                "data {}, cache {}",
                ctx.paths.data_dir.display(),
                ctx.paths.cache_dir.display()
            ),
        ),
        Err(e) => check("paths", Status::Fail, e.to_string()),
    });
    let store = ctx.store();
    checks.push(match store.keyring_status().await {
        KeyringStatus::Available => check("keyring", Status::Ok, "OS keyring available"),
        KeyringStatus::Disabled => check(
            "keyring",
            Status::Ok,
            format!(
                "disabled by LESSDUGIN_KEYRING; sessions are 0600 files in {}",
                store.dir().display()
            ),
        ),
        KeyringStatus::Unavailable(why) => check(
            "keyring",
            Status::Warn,
            format!(
                "no OS keyring ({why}); sessions are 0600 files in {}",
                store.dir().display()
            ),
        ),
    });
    if let Err(e) = ctx.engine() {
        checks.push(check("stores", Status::Fail, format!("{e:?}")));
        return finish(ctx, checks);
    }

    let mut probes = Vec::new();
    let plc = cfg.effective_plc().clone();
    if let Some(u) = join(&plc, "/_health") {
        probes.push(reach(ctx, format!("plc {}", host(&plc)), u));
    }
    let production = !cfg.is_devnet();
    let mut doh_probes = Vec::new();
    let mut infra = Vec::new();
    if production {
        for d in &cfg.doh {
            doh_probes.push(doh(ctx, d.clone()));
        }
        for r in &cfg.relays {
            if let Some(u) = join(r, "/xrpc/_health") {
                infra.push(reach(ctx, format!("relay {}", host(r)), u));
            }
        }
        for j in &cfg.jetstream {
            let mut u = j.clone();
            let _ = u.set_scheme("https");
            u.set_path("/");
            infra.push(reach(ctx, format!("jetstream {}", host(j)), u));
        }
    }
    checks.extend(join_all(probes).await);
    checks.extend(join_all(doh_probes).await);
    checks.extend(join_all(infra).await);

    let account = ctx.account().await.ok();
    let pds = match (&args.pds, &account, &cfg.network) {
        (Some(p), _, _) => Some(p.clone()),
        (None, Some(a), _) => Url::parse(&a.pds).ok(),
        (None, None, NetworkProfile::Devnet(d)) => Some(d.pds.clone()),
        _ => None,
    };
    match pds {
        Some(p) => checks.extend(pds_checks(ctx, p).await),
        None => checks.push(check(
            "pds",
            Status::Skip,
            "not logged in; pass --pds <url> to check a PDS",
        )),
    }
    if let Some(a) = &account {
        checks.push(match store.load(&a.did).await {
            Ok(Some(s)) => {
                let kind = match s {
                    StoredSession::AppPassword(_) => "app password",
                    StoredSession::Oauth(_) => "oauth",
                };
                check(
                    "session",
                    Status::Ok,
                    format!("{} ({kind}) has a stored session", a.did),
                )
            }
            Ok(None) => check(
                "session",
                Status::Warn,
                format!("{} has no stored session: log in again", a.did),
            ),
            Err(e) => check("session", Status::Fail, e.message),
        });
    }
    checks.push(check(
        "range cursor",
        Status::Skip,
        "no host verdicts recorded yet (written by sync from M2)",
    ));
    finish(ctx, checks)
}

fn finish(ctx: &Ctx, checks: Vec<Check>) -> Result<(), CliError> {
    let failed = checks.iter().any(|c| c.status == Status::Fail);
    if ctx.out.is_json() {
        ctx.out.json(&DoctorReport {
            dto: DTO_VERSION,
            checks,
        });
    } else {
        for c in &checks {
            let (mark, style) = match c.status {
                Status::Ok => (lit("ok    "), Style::Good),
                Status::Warn => (lit("warn  "), Style::Warn),
                Status::Fail => (lit("FAIL  "), Style::Bad),
                Status::Skip => (lit("skip  "), Style::Dim),
            };
            ctx.out.line(&[
                (mark, style),
                (tt(&c.name), Style::Bold),
                (cat([lit("  "), tt(&c.detail)]), Style::Plain),
            ]);
        }
    }
    if failed {
        return Err(CliError::Reported(1));
    }
    Ok(())
}
