// SPDX-License-Identifier: AGPL-3.0-or-later
//! The view: a pure function of the [`State`] (`synthesis-v1.md` §6.11).
//!
//! The frame is a header line (where you are, the account, queued writes), an info line (the
//! screen's sort, completeness, sync state, and the "N new — R" banner of a pending live
//! update), the content, and a status line (the last message, or the keys). Every string is a
//! [`TermText`](crate::text::TermText) turned into spans by [`crate::text`].

use lessdugin_core::dto::{ForumRole, MembershipMode, PostSort};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{
    ForumScreen, HomeScreen, LIST_ROWS, Screen, ScreenKind, State, SyncStatus, ThreadScreen, Tone,
    status_text,
};
use crate::keys::HELP;
use crate::text::{
    TermText, actor, banner_lines, cat, clock, joined, line, lit, name, pad, post_meta, post_title,
    spans, tt, wrapped,
};

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn warn_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn selected_gutter() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// The gutter of a row: a bar when it belongs to the selected item.
fn gutter(on: bool) -> (TermText, Style) {
    if on {
        (tt("\u{258c} "), selected_gutter())
    } else {
        (lit("  "), Style::default())
    }
}

/// Draws the whole frame.
pub fn draw(state: &State, f: &mut Frame<'_>) {
    let area = f.area();
    let [header, info, content, status] = Split::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(state.status_rows()),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(header_line(state)).style(header_style()),
        header,
    );
    if let Some(top) = state.top() {
        f.render_widget(Paragraph::new(info_line(state, top)), info);
        let lines = content_lines(state, top, usize::from(content.height));
        f.render_widget(Paragraph::new(Text::from(lines)), content);
    }
    f.render_widget(Paragraph::new(status_lines(state)), status);
    if state.help {
        help(f, state, area);
    }
}

fn header_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

fn header_line(state: &State) -> Line<'static> {
    let mut crumbs = vec![lit(" lessdugin")];
    for s in state.stack.iter().skip(1) {
        let label = match &s.kind {
            ScreenKind::Home(_) => continue,
            ScreenKind::Forum(f) => f.name().map_or_else(|| lit("forum"), name),
            ScreenKind::Thread(t) => t
                .post()
                .map_or_else(|| lit("thread"), |p| post_title(&p.post)),
        };
        crumbs.push(label);
    }
    let mut left = TermText::empty();
    for (i, c) in crumbs.into_iter().enumerate() {
        if i > 0 {
            left.append(tt(" \u{203a} "));
        }
        left.append(c);
    }
    let mut right = Vec::new();
    if state.offline {
        right.push(lit("offline"));
    }
    if !state.writes.is_empty() {
        let n = u32::try_from(state.writes.len()).unwrap_or(u32::MAX);
        right.push(cat([
            crate::text::count(n, "write", "writes"),
            lit(" queued"),
        ]));
    }
    if !state.failed.is_empty() {
        let n = u32::try_from(state.failed.len()).unwrap_or(u32::MAX);
        right.push(cat([
            crate::text::count(n, "write", "writes"),
            lit(" failed"),
        ]));
    }
    right.push(match &state.account {
        Some(a) => match &a.handle {
            Some(h) => cat([lit("@"), name(h)]),
            None => tt(&a.did),
        },
        None => lit("not logged in (read-only)"),
    });
    let right = cat([joined(right), lit(" ")]);
    let width = usize::from(state.width);
    let room = width.saturating_sub(right.width() + 1);
    let left = fit(&left, room);
    let gap = width.saturating_sub(left.width() + right.width());
    line([
        (left, Style::default()),
        (pad(gap), Style::default()),
        (right, Style::default()),
    ])
}

/// `t` cut to `width` columns (with `…` when cut).
fn fit(t: &TermText, width: usize) -> TermText {
    if t.width() <= width {
        return t.clone();
    }
    let s = t.to_plain_string();
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('\u{2026}');
    tt(&out)
}

fn sync_text(s: Option<&SyncStatus>) -> Option<TermText> {
    Some(match s? {
        SyncStatus::Running { done, total } if *total > 0 => {
            cat([lit("syncing "), tt(&format!("{done}/{total}"))])
        }
        SyncStatus::Running { .. } => lit("syncing"),
        SyncStatus::Done {
            at_ms,
            partial: false,
            ..
        } => cat([lit("synced "), tt(&clock(*at_ms))]),
        SyncStatus::Done {
            at_ms,
            complete,
            total,
            ..
        } => cat([
            lit("synced "),
            tt(&clock(*at_ms)),
            lit(" ("),
            tt(&format!("{complete} of {total}")),
            lit(" repositories)"),
        ]),
        SyncStatus::Finished => lit("synced"),
        SyncStatus::Failed { message, .. } => cat([lit("sync failed: "), message.clone()]),
    })
}

fn banner(top: &Screen) -> Option<TermText> {
    let n = top.pending_new()?;
    Some(if n > 0 {
        tt(&format!(" {n} new \u{2014} R "))
    } else {
        tt(" updated \u{2014} R ")
    })
}

fn banner_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn info_line(state: &State, top: &Screen) -> Line<'static> {
    let mut parts: Vec<(TermText, Style)> = Vec::new();
    if let Some(b) = banner(top) {
        parts.push((b, banner_style()));
        parts.push((lit(" "), Style::default()));
    }
    let mut info: Vec<TermText> = Vec::new();
    match &top.kind {
        ScreenKind::Home(_) => info.push(lit("Forums you follow")),
        ScreenKind::Forum(f) => {
            let tabs = [
                (PostSort::Magic, "magic"),
                (PostSort::Top, "top"),
                (PostSort::New, "new"),
                (PostSort::Old, "old"),
            ];
            parts.push((lit(" sort: "), dim()));
            for (sort, label) in tabs {
                let style = if f.sort == sort {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    dim()
                };
                parts.push((cat([lit(" "), lit(label), lit(" ")]), style));
            }
            parts.push((lit("  filter: frontpage "), dim()));
            if let Some(d) = &f.data {
                let c = &d.list.completeness;
                if c.partial || c.degraded {
                    info.push(cat([
                        tt(&format!("{} of {}", c.repos_complete, c.repos_total)),
                        lit(" repositories"),
                        lit(if c.degraded {
                            ", moderation incomplete"
                        } else {
                            ""
                        }),
                    ]));
                }
            }
            if let Some(s) = sync_text(state.sync.get(&f.uri)) {
                info.push(s);
            }
        }
        ScreenKind::Thread(t) => {
            if let Some(p) = t.post() {
                if let Some(n) = forum_name(state, &p.post.forum) {
                    info.push(n);
                }
                if let Some(d) = &t.data {
                    let c = &d.view.completeness;
                    if c.partial || c.degraded {
                        info.push(cat([
                            tt(&format!("{} of {}", c.repos_complete, c.repos_total)),
                            lit(" repositories"),
                            lit(if c.degraded {
                                ", moderation incomplete"
                            } else {
                                ""
                            }),
                        ]));
                    }
                }
                if let Some(s) = sync_text(state.sync.get(&p.post.forum)) {
                    info.push(s);
                }
            }
        }
    }
    if !info.is_empty() {
        parts.push((cat([lit(" "), joined(info)]), dim()));
    }
    line(parts)
}

fn forum_name(state: &State, uri: &str) -> Option<TermText> {
    state.stack.iter().find_map(|s| match &s.kind {
        ScreenKind::Forum(f) if f.uri == uri => f.name().map(name),
        ScreenKind::Home(h) => h
            .data
            .as_ref()?
            .forums
            .iter()
            .find(|f| f.uri == uri)
            .map(|f| name(&f.name)),
        _ => None,
    })
}

/// The status message (wrapped to [`State::status_rows`] rows), or the keys.
fn status_lines(state: &State) -> Text<'static> {
    if let Some(s) = &state.status {
        let style = match s.tone {
            Tone::Info => Style::default(),
            Tone::Good => Style::default().fg(Color::Green),
            Tone::Warn => Style::default().fg(Color::Yellow),
            Tone::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        };
        let mut lines = wrapped(&status_text(s), style, usize::from(state.width));
        lines.truncate(usize::from(state.status_rows()));
        return Text::from(lines);
    }
    let hints = match state.top().map(|s| &s.kind) {
        Some(ScreenKind::Home(_)) => {
            " Enter open \u{b7} R sync \u{b7} P post \u{b7} ? help \u{b7} q quit"
        }
        Some(ScreenKind::Forum(_)) => {
            " Enter open \u{b7} s sort \u{b7} P post \u{b7} R sync \u{b7} ? help \u{b7} h back"
        }
        Some(ScreenKind::Thread(_)) => {
            " r reply \u{b7} c comment \u{b7} Space fold \u{b7} p parent \u{b7} R sync \u{b7} ? help \u{b7} h back"
        }
        None => "",
    };
    Text::from(line([(tt(hints), dim())]))
}

fn message(lines: &mut Vec<Line<'static>>, t: TermText, style: Style) {
    lines.push(line([(lit("  "), Style::default()), (t, style)]));
}

fn content_lines(state: &State, top: &Screen, h: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match &top.kind {
        ScreenKind::Home(s) => home(state, top, s, h, &mut lines),
        ScreenKind::Forum(s) => forum(top, s, h, &mut lines),
        ScreenKind::Thread(t) => thread(top, t, h, &mut lines),
    }
    lines
}

fn loading_or_error(top: &Screen, lines: &mut Vec<Line<'static>>) {
    match &top.error {
        Some(e) => message(
            lines,
            cat([lit("Could not load: "), tt(&e.message)]),
            Style::default().fg(Color::Red),
        ),
        None => message(lines, lit("Loading..."), dim()),
    }
}

fn home(state: &State, top: &Screen, s: &HomeScreen, h: usize, lines: &mut Vec<Line<'static>>) {
    let Some(d) = &s.data else {
        loading_or_error(top, lines);
        return;
    };
    if d.forums.is_empty() {
        lines.push(Line::default());
        message(lines, lit("You follow no forum yet."), bold());
        message(lines, lit("Join one with `lessdugin join <forum>`,"), dim());
        message(
            lines,
            lit("or open one with `lessdugin tui <forum>`."),
            dim(),
        );
        return;
    }
    let visible = (h / LIST_ROWS).max(1);
    for (i, f) in d.forums.iter().enumerate().skip(s.pos.top).take(visible) {
        let on = i == s.pos.cursor;
        let title_style = if on {
            bold().add_modifier(Modifier::REVERSED)
        } else {
            bold()
        };
        let mut title = vec![gutter(on), (name(&f.name), title_style)];
        if let Some(a) = &f.alias {
            title.push((cat([lit("  ("), name(a), lit(")")]), dim()));
        }
        lines.push(line(title));
        let mut meta = vec![
            actor(&f.owner),
            lit(match f.membership {
                MembershipMode::Open => "open",
                MembershipMode::Approval => "approval",
            }),
        ];
        if f.member {
            meta.push(lit("member"));
        }
        if let Some(sync) = sync_text(state.sync.get(&f.uri)) {
            meta.push(sync);
        }
        if let Some(desc) = &f.description {
            meta.push(tt(desc));
        }
        lines.push(line([gutter(on), (joined(meta), dim())]));
    }
}

fn forum(top: &Screen, s: &ForumScreen, h: usize, lines: &mut Vec<Line<'static>>) {
    let Some(d) = &s.data else {
        loading_or_error(top, lines);
        return;
    };
    for b in banner_lines(&d.list.banners) {
        lines.push(line([(lit("  "), Style::default()), (b, warn_style())]));
    }
    if d.list.items.is_empty() {
        lines.push(Line::default());
        message(lines, lit("No posts yet."), bold());
        let hint = match d.view.my_role {
            ForumRole::None => lit("Join the forum with `lessdugin join` to post in it."),
            _ => lit("Press P to write the first one."),
        };
        message(lines, hint, dim());
        return;
    }
    let visible = (h / LIST_ROWS).max(1);
    for (i, p) in d
        .list
        .items
        .iter()
        .enumerate()
        .skip(s.pos.top)
        .take(visible)
    {
        let on = i == s.pos.cursor;
        let title_style = if on {
            bold().add_modifier(Modifier::REVERSED)
        } else {
            bold()
        };
        lines.push(line([gutter(on), (post_title(p), title_style)]));
        lines.push(line([gutter(on), (joined(post_meta(p)), dim())]));
    }
}

fn thread(top: &Screen, t: &ThreadScreen, h: usize, lines: &mut Vec<Line<'static>>) {
    if t.data.is_none() {
        loading_or_error(top, lines);
        return;
    }
    let l = &t.layout;
    let sel = l.items.get(t.pos.selected);
    for (n, row) in l.lines.iter().enumerate().skip(t.pos.scroll).take(h) {
        let on = sel.is_some_and(|it| n >= it.start && n < it.end);
        let (g, gs) = gutter(on);
        let mut v = spans(&g, gs);
        v.extend(row.spans.iter().cloned());
        lines.push(Line::from(v));
    }
}

fn help(f: &mut Frame<'_>, state: &State, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (group, rows) in HELP {
        lines.push(line([(tt(group), bold())]));
        for (keys, what) in *rows {
            lines.push(line([
                (lit("  "), Style::default()),
                (tt(&format!("{keys:<16}")), Style::default().fg(Color::Cyan)),
                (tt(what), Style::default()),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(line([(lit("Any key closes this help."), dim())]));
    let width = 64.min(area.width);
    let total = lines.len();
    let height = u16::try_from(total + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    // the content rows inside the border, and the scroll window
    let inner = usize::from(height.saturating_sub(2));
    let max_scroll = total.saturating_sub(inner);
    let scroll = state.help_scroll.min(max_scroll);
    let shown: Vec<Line<'static>> = lines.into_iter().skip(scroll).take(inner).collect();
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let r = Rect::new(x, y, width, height);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(line([(lit(" Keys "), bold())]));
    if max_scroll > 0 {
        block = block.title_bottom(line([(
            tt(&format!(
                " {}-{} of {} ",
                scroll + 1,
                scroll + shown.len(),
                total
            )),
            dim(),
        )]));
    }
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(Text::from(shown)).block(block), r);
}
