// SPDX-License-Identifier: AGPL-3.0-or-later
//! Human output of the forum views (`threat-model.md` TERM-1): every string from the network
//! goes through [`TermText`] ([`crate::out::tt`], [`crate::out::name`]), and bodies through the
//! LFM/1 terminal projection (`StyledDoc`) and its printer (`lfm-1.md` §9.4), which emits only
//! SGR (and OSC 8 links on a colour terminal).
//!
//! Bodies are rendered from the markdown source of the cached record when its CID is the one
//! the view shows; otherwise (another version is displayed, or the body is not inline) the
//! view's plain text P is printed, with its notices.

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::dto::{
    ActorView, Anchor, Badges, CollapseReason, HandleState, HiddenReason, ItemState, KarmaValue,
    PostTag, ReactTally, RenderedBody, ScoreView, Stance,
};
use lessdugin_core::host::Host;
use lessdugin_core::store::records::RecordStore;
use lessdugin_lexicon::collection::nsid;
use lessdugin_markdown::printer::{PrintOpts, print_doc_with};
use lessdugin_markdown::{MediaItem, RenderOpts};
use serde_json::Value;

use crate::out::{Style, TermText, cat, lit, name, tt};

/// The widest text column used, also on very wide terminals.
pub const MAX_WIDTH: usize = 100;

/// The terminal width for wrapping (80 when stdout is not a terminal).
pub fn width(tty: bool) -> usize {
    if tty && let Ok((w, _)) = crossterm::terminal::size() {
        return usize::from(w).clamp(20, MAX_WIDTH);
    }
    80
}

/// `YYYY-MM-DD HH:MM UTC` for a time in ms since the epoch.
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

/// An actor: `@handle` when verified both ways, else the short DID (with the claimed handle
/// marked as unverified).
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

/// The badges as words (`edited`, `pending`, …).
pub fn badges(b: &Badges) -> Vec<&'static str> {
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
            v.push(label);
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

/// An item's score for a meta line (`social.md` §7 "Display"): the points; the agreement when
/// the forum has the axis and it moved; "n votes not yet counted" (voters outside E); and "N of
/// M votes were cast on an earlier version".
pub fn score(s: &ScoreView) -> Vec<TermText> {
    let mut v = vec![cat([
        tt(&s.score.to_string()),
        lit(if s.score.abs() == 1 {
            " point"
        } else {
            " points"
        }),
    ])];
    if let Some(a) = s.agreement
        && a != 0
    {
        v.push(cat([lit("agreement "), tt(&format!("{a:+}"))]));
    }
    if s.unweighted > 0 {
        v.push(cat([
            count(s.unweighted, "vote", "votes"),
            lit(" not yet counted"),
        ]));
    }
    if s.earlier_version_votes > 0 {
        v.push(tt(&format!(
            "{} of {} votes were cast on an earlier version",
            s.earlier_version_votes, s.weighted_votes
        )));
    }
    v
}

/// A post view's tags (`PostView.tagRelevance`, `social.md` §13.3): each slug with its
/// relevance, whether it applies, and the viewer's own tag vote; `None` when it lists none.
pub fn tag_relevance(tags: &[PostTag]) -> Option<TermText> {
    if tags.is_empty() {
        return None;
    }
    let mut parts = vec![lit("tags: ")];
    for (i, t) in tags.iter().enumerate() {
        if i > 0 {
            parts.push(tt(" · "));
        }
        parts.push(cat([lit("#"), name(&t.slug)]));
        parts.push(tt(&format!(" relevance {}", t.relevance)));
        if !t.applies {
            parts.push(lit(" (not applied)"));
        }
        if let Some(v) = t.mine {
            parts.push(cat([
                lit(" [your vote: "),
                lit(match v {
                    KarmaValue::Up => "up",
                    KarmaValue::StrongUp => "strong up",
                    KarmaValue::Down => "down",
                    KarmaValue::StrongDown => "strong down",
                }),
                lit("]"),
            ]));
        }
    }
    Some(cat(parts))
}

/// One react tally as it is listed under its item (`social.md` §9.2 "Display", "Anchors"): the
/// glyph and label, who created it (`handle_of` names a DID's verified handle, else the short
/// DID), how many seconded it and how many are against, its quote (or that the quoted text is
/// no longer present), the viewer's stance, and whether the viewer's own react is not counted.
pub fn react(t: &ReactTally, handle_of: &dyn Fn(&str) -> Option<String>) -> TermText {
    let mut parts = vec![tt(&t.glyph), lit(" "), name(&t.label)];
    if let Some(c) = &t.creator {
        parts.push(lit(" by "));
        parts.push(match handle_of(c) {
            Some(h) => cat([lit("@"), name(&h)]),
            None => tt(&lessdugin_core::identity::short_did(c)),
        });
    }
    let seconded = t.for_count.saturating_sub(1);
    if seconded > 0 {
        parts.push(cat([lit(", "), count(seconded, "seconded", "seconded")]));
    }
    if t.against > 0 {
        parts.push(cat([
            lit(", "),
            tt(&t.against.to_string()),
            lit(" against"),
        ]));
    }
    if let Some(q) = &t.quote {
        if matches!(t.anchor, Some(Anchor::Orphan { .. })) {
            parts.push(lit(" (quoted text no longer present)"));
        } else {
            parts.push(cat([
                lit(" on "),
                tt("\u{201c}"),
                tt(&q.exact),
                tt("\u{201d}"),
            ]));
        }
    }
    match t.mine {
        Some(Stance::For) => parts.push(lit(" [yours]")),
        Some(Stance::Against) => parts.push(lit(" [yours, against]")),
        None => {}
    }
    if t.uncounted {
        parts.push(lit(" (not counted)"));
    }
    cat(parts)
}

/// How bodies are printed.
#[derive(Clone, Copy, Debug)]
pub struct BodyStyle {
    /// SGR styling.
    pub color: bool,
    /// OSC 8 links (colour on a terminal).
    pub hyperlinks: bool,
    /// The full line width.
    pub width: usize,
}

/// Indents every non-empty line by `n` spaces.
pub fn indent(text: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    let mut out = String::with_capacity(text.len() + n * 8);
    for line in text.lines() {
        if !line.is_empty() {
            out.push_str(&pad);
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Renders an `NS.markdown` value (inline text only) for the terminal; `None` for any other
/// value (a spilled or unknown body).
pub fn markdown(
    md: &Value,
    style: &BodyStyle,
    indent_by: usize,
    handle_of: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let ty = md.get("$type")?.as_str()?;
    if ty != nsid::MARKDOWN && ty.strip_suffix("#main") != Some(nsid::MARKDOWN) {
        return None;
    }
    let text = md.get("text")?.as_str()?;
    let media: Vec<MediaItem> = md
        .get("media")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| serde_json::from_value(m.clone()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let profile = md.get("profile").and_then(Value::as_str);
    let opts = RenderOpts {
        html: false,
        styled: true,
        text: false,
    };
    let r = lessdugin_markdown::render_profile(text, &media, profile, &opts);
    let print = PrintOpts {
        color: style.color,
        hyperlinks: style.hyperlinks && style.color,
        reveal_spoilers: false,
        width: Some(style.width.saturating_sub(indent_by).max(20)),
    };
    let s = print_doc_with(&r.styled, &print, handle_of);
    Some(indent(s.trim_end_matches('\n'), indent_by))
}

/// The body of `uri` as the view shows it: the cached record's markdown when its CID is `cid`,
/// else the view's plain text and notices.
pub async fn body<H: Host>(
    host: &H,
    uri: &str,
    cid: &str,
    view: &RenderedBody,
    style: &BodyStyle,
    indent_by: usize,
    handle_of: &dyn Fn(&str) -> Option<String>,
) -> String {
    if let Ok(u) = AtUri::parse(uri)
        && let Ok(Some(row)) = RecordStore::new(host.cache()).get(&u).await
        && row.cid.to_string() == cid
        && let Ok(v) = row.value_json()
        && let Some(md) = v.get("body")
        && let Some(s) = markdown(md, style, indent_by, handle_of)
    {
        return with_notices(s, view, style, indent_by);
    }
    view_body(view, style, indent_by)
}

/// A rendered body as the view gives it: its plain text and notices (a body the CLI does not
/// render from its markdown, such as a spilled one).
pub fn view_body(view: &RenderedBody, style: &BodyStyle, indent_by: usize) -> String {
    let mut s = String::new();
    if !view.text.is_empty() {
        s = indent(
            &tt(&view.text).to_plain_string_styled(style.color),
            indent_by,
        );
    }
    with_notices(s, view, style, indent_by)
}

fn with_notices(mut s: String, view: &RenderedBody, style: &BodyStyle, indent_by: usize) -> String {
    for n in &view.notices {
        let line = crate::out::render(&tt(n), Style::Dim, style.color);
        s.push_str(&indent(&line, indent_by));
    }
    s
}

/// Rendering helpers on [`TermText`].
pub trait TermTextExt {
    /// The text with escaped runs in reverse video when `color` (as the printer shows them).
    fn to_plain_string_styled(&self, color: bool) -> String;
}

impl TermTextExt for TermText {
    fn to_plain_string_styled(&self, color: bool) -> String {
        crate::out::render(self, Style::Plain, color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(time(0), "1970-01-01 00:00 UTC");
        assert_eq!(time(951_782_400_000), "2000-02-29 00:00 UTC");
        assert_eq!(time(1_792_627_200_000 + 3_723_000), "2026-10-22 01:02 UTC");
        assert_eq!(time(-1), "1969-12-31 23:59 UTC");
    }

    #[test]
    fn markdown_bodies_are_sanitised() {
        let md = serde_json::json!({
            "$type": nsid::MARKDOWN,
            "text": "# Hi\n\nA \u{1b}]0;x\u{7} [link](https://example.com).",
        });
        let style = BodyStyle {
            color: false,
            hyperlinks: false,
            width: 80,
        };
        let s = markdown(&md, &style, 2, &|_| None).unwrap();
        assert!(!s.contains('\u{1b}') && !s.contains('\u{7}'), "{s:?}");
        assert!(s.starts_with("  "), "{s:?}");
        assert!(s.contains("example.com"), "{s:?}");
        let other = serde_json::json!({"$type": "x.y", "text": "a"});
        assert!(markdown(&other, &style, 0, &|_| None).is_none());
    }
}
