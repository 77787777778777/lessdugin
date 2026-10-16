// SPDX-License-Identifier: AGPL-3.0-or-later
//! `resolve` (`sync.md` §3) and `verify` (`sync.md` §8.8).

use lessdugin_core::dto::{HandleState, VerifyOutcome};
use lessdugin_core::identity::IdentityLevel;
use lessdugin_core::verify::DEEP_SKIPPED;

use crate::args::{ResolveArgs, VerifyArgs};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, cat, dto, lit, name, tt};

/// `lessdugin resolve <handle|did>`: prints `ResolvedActor` with `--json`.
pub async fn resolve(ctx: &Ctx, args: ResolveArgs) -> Result<(), CliError> {
    let engine = ctx.engine()?;
    let r = engine.resolve_identity(&args.actor).await?;
    if ctx.out.is_json() {
        ctx.out.json(&dto(&r));
        return Ok(());
    }
    let id = &r.identity;
    let out = &ctx.out;
    out.line(&[(tt(id.did.as_str()), Style::Bold)]);
    let handle = match (&id.claimed_handle, id.handle_state) {
        (Some(h), HandleState::Verified) => cat([name(h), lit(" (verified both ways)")]),
        (Some(h), HandleState::Unverified) => cat([
            name(h),
            lit(" (claimed, not verified: not shown as the handle)"),
        ]),
        (Some(h), HandleState::Invalid) => cat([name(h), lit(" (invalid)")]),
        (None, _) => lit("(none)"),
    };
    out.field("handle", 11, handle);
    if id.dns_unauthenticated {
        out.field(
            "dns",
            11,
            lit("unauthenticated (system resolver, not DNS over HTTPS)"),
        );
    }
    match (&id.pds, &id.pds_problem) {
        (Some(p), _) => out.field("pds", 11, tt(p)),
        (None, Some(why)) => out.field("pds", 11, cat([lit("none: "), tt(why)])),
        (None, None) => out.field("pds", 11, lit("none")),
    }
    match (&id.signing_key, &id.key_problem) {
        (Some(k), _) => out.field("signing key", 11, tt(k)),
        (None, Some(why)) => out.field("signing key", 11, cat([lit("none: "), tt(why)])),
        (None, None) => out.field("signing key", 11, lit("none")),
    }
    let level = match id.level {
        IdentityLevel::Doc => lit("DID document"),
        IdentityLevel::Hint => lit("hint only (no DID document)"),
    };
    out.field("level", 11, level);
    if let Some(m) = &id.handle_method {
        let m = serde_json::to_value(m)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        out.field("handle via", 11, tt(&m));
    }
    Ok(())
}

/// `lessdugin verify <at-uri> [--deep]`: prints `VerifyReport` with `--json`; exits 1 when
/// the proof fails.
pub async fn verify(ctx: &Ctx, args: VerifyArgs) -> Result<(), CliError> {
    let engine = ctx.engine()?;
    let report = engine.verify(&args.uri, args.deep).await?;
    let failed = report.result == VerifyOutcome::Failed;
    if ctx.out.is_json() {
        ctx.out.json(&report);
    } else {
        let out = &ctx.out;
        let (label, style) = match report.result {
            VerifyOutcome::Proven => (lit("proven"), Style::Good),
            VerifyOutcome::Absent => (lit("proven absent"), Style::Good),
            VerifyOutcome::Failed => (lit("FAILED"), Style::Bad),
        };
        out.line(&[
            (label, style),
            (lit("  "), Style::Plain),
            (tt(&report.uri), Style::Bold),
        ]);
        if let Some(rev) = &report.rev {
            out.field("rev", 6, tt(rev));
        }
        if let Some(c) = &report.commit_cid {
            out.field("commit", 6, tt(c));
        }
        out.field("key", 6, tt(&report.key_source));
        for c in &report.checks {
            let skipped = c.ok && c.detail.as_deref() == Some(DEEP_SKIPPED);
            let (mark, st) = match (c.ok, skipped) {
                (_, true) => (lit("  skip  "), Style::Dim),
                (true, false) => (lit("  ok    "), Style::Good),
                (false, _) => (lit("  FAIL  "), Style::Bad),
            };
            let mut spans = vec![(mark, st), (tt(&c.name), Style::Plain)];
            if let Some(d) = &c.detail {
                spans.push((cat([lit(": "), tt(d)]), Style::Dim));
            }
            out.line(&spans);
        }
    }
    if failed {
        return Err(CliError::Reported(1));
    }
    Ok(())
}
