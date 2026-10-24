// SPDX-License-Identifier: AGPL-3.0-or-later
//! Terminal text (`threat-model.md` TERM-1, TERM-2, TERM-5).
//!
//! Every string the TUI draws is a [`TermText`]: fixed labels via [`lit`] (printable ASCII),
//! everything else via [`term_sanitize`] ([`tt`], [`name`]). The only way this crate builds a
//! ratatui [`Span`] is [`spans`]/[`line`] from a `TermText` (escaped runs in reverse video), and
//! [`crate::body`] for the LFM/1 printer's output, which is itself made of `TermText` runs and
//! checked before use. Nothing relies on ratatui to filter control characters.

use lessdugin_core::dto::{
    ActorView, AgreementValue, Badges, Banner, CollapseReason, HandleState, HiddenReason,
    ItemState, KarmaValue, PostKind, PostSummary, ScoreView,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub use lessdugin_markdown::term::{RunKind, TermCtx, TermText, term_sanitize};

/// Sanitised prose (messages, URLs, identifiers, titles).
pub fn tt(s: &str) -> TermText {
    term_sanitize(s, TermCtx::Prose)
}

/// A sanitised name (handles, display names, forum names): whitespace collapsed, invisible
/// characters and badge glyphs removed, at most 256 graphemes.
pub fn name(s: &str) -> TermText {
    term_sanitize(s, TermCtx::Name { max_graphemes: 256 })
}

/// A fixed label (printable ASCII; panics otherwise).
pub fn lit(s: &'static str) -> TermText {
    TermText::from_static(s)
}

/// Concatenates texts.
pub fn cat(parts: impl IntoIterator<Item = TermText>) -> TermText {
    let mut t = TermText::empty();
    for p in parts {
        t.append(p);
    }
    t
}

/// Joins texts with ` · `.
pub fn joined(parts: impl IntoIterator<Item = TermText>) -> TermText {
    let mut out = TermText::empty();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.append(tt(" \u{b7} "));
        }
        out.append(p);
    }
    out
}

/// The spans of `t` in `style`; escaped runs (visible stand-ins for controls) in reverse video.
pub fn spans(t: &TermText, style: Style) -> Vec<Span<'static>> {
    t.runs()
        .iter()
        .map(|r| match r.kind() {
            RunKind::Plain => Span::styled(r.text().to_owned(), style),
            RunKind::Escaped => {
                Span::styled(r.text().to_owned(), style.add_modifier(Modifier::REVERSED))
            }
        })
        .collect()
}

/// One line of styled texts.
pub fn line(parts: impl IntoIterator<Item = (TermText, Style)>) -> Line<'static> {
    let mut v = Vec::new();
    for (t, s) in parts {
        v.extend(spans(&t, s));
    }
    Line::from(v)
}

/// `t` in `style`, wrapped by character at `width` columns (a status message, which has no
/// layout of its own); at least one line.
pub fn wrapped(t: &TermText, style: Style, width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for sp in spans(t, style) {
        let mut buf = String::new();
        for c in sp.content.chars() {
            let w = c.width().unwrap_or(0);
            if used > 0 && used + w > width {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), sp.style));
                }
                lines.push(Line::from(std::mem::take(&mut cur)));
                used = 0;
            }
            buf.push(c);
            used += w;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, sp.style));
        }
    }
    lines.push(Line::from(cur));
    lines
}

/// `n` spaces (an indent).
pub fn pad(n: usize) -> TermText {
    tt(&" ".repeat(n))
}

/// `YYYY-MM-DD HH:MM UTC` for a time in ms since the epoch (as the CLI prints it).
pub fn time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        sod / 3600,
        (sod % 3600) / 60
    )
}

/// `HH:MM UTC`.
pub fn clock(ms: i64) -> String {
    let sod = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02} UTC", sod / 3600, (sod % 3600) / 60)
}

/// The civil date of a day number (days since 1970-01-01), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// An actor: `@handle` when verified both ways, else the short DID (with a claimed handle
/// marked as unverified), after the display name if there is one.
pub fn actor(a: &ActorView) -> TermText {
    let who = match (&a.handle, a.handle_state) {
        (Some(h), _) => cat([lit("@"), name(h)]),
        (None, HandleState::Invalid) => cat([tt(&a.short_did), lit(" (invalid handle)")]),
        (None, _) => match &a.claimed_handle {
            Some(c) => cat([
                tt(&a.short_did),
                lit(" (claims @"),
                name(c),
                lit(", unverified)"),
            ]),
            None => tt(&a.short_did),
        },
    };
    match &a.display_name {
        Some(n) if !n.trim().is_empty() => cat([name(n), lit(" "), who]),
        _ => who,
    }
}

/// The forum's banners (`social.md` §1.6, §4.11; `moderationUnavailable`, `newerRules`), one
/// line each. Rendered on the forum/list/thread screens (`synthesis-v1.md` §6.11).
pub fn banner_lines(banners: &[Banner]) -> Vec<TermText> {
    banners
        .iter()
        .map(|b| cat([lit("! "), tt(&b.message)]))
        .collect()
}

fn hidden_reason(r: &HiddenReason) -> TermText {
    match r {
        HiddenReason::NotParticipant => lit("not a participant"),
        HiddenReason::ModerationUnavailable => lit("moderation unavailable"),
        HiddenReason::Removed { reason, .. } => match reason {
            Some(why) => cat([lit("removed by a moderator: "), tt(why)]),
            None => lit("removed by a moderator"),
        },
        HiddenReason::Banned => lit("author banned"),
        HiddenReason::Restricted => lit("author restricted"),
        HiddenReason::AuthorModerated => lit("author moderated"),
        HiddenReason::Locked => lit("thread locked"),
        HiddenReason::NewUser => lit("new user"),
    }
}

/// A short label for a state other than visible.
pub fn state_label(s: &ItemState) -> Option<TermText> {
    match s {
        ItemState::Visible => None,
        ItemState::Collapsed { reason } => Some(cat([
            lit("collapsed: "),
            lit(match reason {
                CollapseReason::NewUser => "new user",
                CollapseReason::LowScore => "low score",
                CollapseReason::Muted => "muted",
            }),
        ])),
        ItemState::Hidden { reason } => Some(cat([lit("hidden: "), hidden_reason(reason)])),
        ItemState::Tombstone { reason } => Some(cat([lit("deleted: "), hidden_reason(reason)])),
    }
}

/// The badges as fixed words (the badge column, TERM-5).
pub fn badges(b: &Badges) -> Vec<TermText> {
    let mut v = Vec::new();
    for (on, label) in [
        (b.verified, "verified"),
        (b.edited, "edited"),
        (b.pending, "pending"),
        (b.future_dated, "future-dated"),
        (b.identity_unverified, "identity unverified"),
        (b.stale, "stale"),
        (b.hint, "unconfirmed"),
        (b.inconsistent_timestamps, "inconsistent timestamps"),
    ] {
        if on {
            v.push(lit(label));
        }
    }
    v
}

/// `n thing`/`n things`.
pub fn count(n: u32, one: &'static str, many: &'static str) -> TermText {
    cat([
        tt(&n.to_string()),
        lit(" "),
        lit(if n == 1 { one } else { many }),
    ])
}

/// `n point(s)`.
pub fn points(s: &ScoreView) -> TermText {
    cat([
        tt(&s.score.to_string()),
        lit(if s.score.abs() == 1 {
            " point"
        } else {
            " points"
        }),
    ])
}

/// `agreement +n` / `-n` (only when the forum has an agreement axis and it moved; a zero
/// agreement would only bloat every meta line).
pub fn agreement(s: &ScoreView) -> Option<TermText> {
    let n = s.agreement?;
    if n == 0 {
        return None;
    }
    Some(cat([lit("agreement "), tt(&format!("{:+}", n))]))
}

/// The viewer's own vote as one word (`your: up`, `your: strong agree`).
fn own_word(mine: &lessdugin_core::dto::MyOpinion) -> Vec<TermText> {
    let mut v = Vec::new();
    if let Some(k) = &mine.karma {
        v.push(lit(match k {
            KarmaValue::Up => "up",
            KarmaValue::StrongUp => "strong up",
            KarmaValue::Down => "down",
            KarmaValue::StrongDown => "strong down",
        }));
    }
    if let Some(a) = &mine.agreement {
        if !v.is_empty() {
            v.push(lit(", "));
        }
        v.push(lit(match a {
            AgreementValue::Agree => "agree",
            AgreementValue::StrongAgree => "strong agree",
            AgreementValue::Disagree => "disagree",
            AgreementValue::StrongDisagree => "strong disagree",
        }));
    }
    v
}

/// The viewer's own vote with its pending/not-established markers (`social.md` §7,
/// `engine-api.md` §3.5): `your: up [pending]` / `your: up (not counted yet)`.
pub fn mine(m: &lessdugin_core::dto::MyOpinion) -> Option<TermText> {
    let words = own_word(m);
    if words.is_empty() && !m.pending {
        return None;
    }
    let mut parts = vec![lit("your:")];
    if !words.is_empty() {
        parts.push(lit(" "));
        parts.extend(words);
    }
    if m.pending {
        parts.push(lit(" [pending]"));
    } else if m.not_yet_counted {
        parts.push(lit(" (not counted yet)"));
    }
    Some(cat(parts))
}

/// The score's auxiliary counts (`social.md` §7): "n votes not yet counted" (`unweighted`) and
/// "N of M votes were cast on an earlier version".
pub fn score_notes(s: &ScoreView) -> Vec<TermText> {
    let mut v = Vec::new();
    if s.unweighted > 0 {
        v.push(cat([
            count(s.unweighted, "vote", "votes"),
            lit(" not yet counted"),
        ]));
    }
    if s.earlier_version_votes > 0 {
        v.push(cat([tt(&format!(
            "{} of {} votes were cast on an earlier version",
            s.earlier_version_votes, s.weighted_votes
        ))]));
    }
    v
}

/// Every part of an item's score for a meta line: the points, agreement, the auxiliary
/// counts and the viewer's own vote (with its markers).
pub fn score_parts(s: &ScoreView) -> Vec<TermText> {
    let mut v = vec![points(s)];
    if let Some(a) = agreement(s) {
        v.push(a);
    }
    v.extend(score_notes(s));
    if let Some(m) = s.mine.as_ref().and_then(mine) {
        v.push(m);
    }
    v
}

/// A post's title: `[deleted]`/`[hidden]`, the title, or the first line of the excerpt.
pub fn post_title(p: &PostSummary) -> TermText {
    match (&p.title, &p.state) {
        (_, ItemState::Tombstone { .. }) => lit("[deleted]"),
        (_, ItemState::Hidden { .. }) => lit("[hidden]"),
        (Some(t), _) if !t.trim().is_empty() => tt(t),
        _ => {
            let first = p.excerpt.lines().next().unwrap_or_default();
            if first.is_empty() {
                lit("(untitled)")
            } else {
                tt(first)
            }
        }
    }
}

/// A post's meta line: author, time, comments, score (with agreement, notes and the viewer's
/// own vote), kind, badges (tier, pinned, locked, unread), tags, state and trust badges.
pub fn post_meta(p: &PostSummary) -> Vec<TermText> {
    let mut parts = vec![
        actor(&p.author),
        tt(&time(p.created_at_ms)),
        count(p.comment_count, "comment", "comments"),
    ];
    parts.extend(score_parts(&p.score));
    match p.post_kind {
        PostKind::Article => {}
        PostKind::Link => parts.push(lit("link")),
        PostKind::Question => parts.push(lit("question")),
        PostKind::QuickTake => parts.push(lit("quick take")),
    }
    match p.tier {
        lessdugin_core::dto::Tier::Curated => parts.push(lit("curated")),
        lessdugin_core::dto::Tier::Frontpage | lessdugin_core::dto::Tier::Personal => {}
    }
    if p.pinned {
        parts.push(lit("pinned"));
    }
    if p.locked {
        parts.push(lit("locked"));
    }
    if let Some(n) = p.unread_comments
        && n > 0
    {
        parts.push(cat([count(n, "unread", "unread")]));
    }
    if !p.tags.is_empty() {
        parts.push(cat(p.tags.iter().enumerate().map(|(i, t)| {
            cat([if i == 0 { lit("#") } else { lit(" #") }, name(t)])
        })));
    }
    if let Some(s) = state_label(&p.state) {
        parts.push(s);
    }
    parts.extend(badges(&p.badges));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_never_reach_spans() {
        let t = tt("a\u{1b}]0;title\u{7}b\u{202e}c");
        let s = spans(&t, Style::default());
        let joined: String = s.iter().map(|s| s.content.as_ref()).collect();
        assert!(!joined.chars().any(char::is_control), "{joined:?}");
        assert!(
            s.iter()
                .any(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "escaped runs are reversed"
        );
    }

    #[test]
    fn times() {
        assert_eq!(time(0), "1970-01-01 00:00 UTC");
        assert_eq!(clock(1_792_627_200_000 + 3_723_000), "01:02 UTC");
    }

    #[test]
    fn names_lose_badge_glyphs() {
        let t = name("Mallory \u{2705} [verified]");
        assert!(!t.to_plain_string().contains('\u{2705}'));
    }
}
