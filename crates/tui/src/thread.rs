// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Thread screen: the post and its comment tree laid out as lines, and the navigation over
//! them (`docs/design/synthesis-v1.md` §6.11).
//!
//! The layout depends only on the data, the collapsed comments and the width; the selection
//! is drawn by the view in a two-column gutter, so moving it never re-lays out the thread.
//! Moving down through an item taller than the viewport scrolls it line by line first.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_core::dto::{CommentNode, ItemState, ThreadRoot};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::app::ThreadData;
use crate::text::{
    TermText, actor, badges, banner_lines, count, joined, line, lit, pad, post_meta, post_title,
    score_parts, state_label, time, tt,
};

/// The selection gutter, in columns.
pub const GUTTER: u16 = 2;

/// The widest a body is wrapped, also on very wide terminals (as the CLI).
pub const MAX_BODY: usize = 100;

/// One item's lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemSpan {
    /// The post's or comment's uri.
    pub uri: String,
    /// The first line.
    pub start: usize,
    /// One past the last line.
    pub end: usize,
    /// The parent comment (comments only; `None` for top-level comments and the post).
    pub parent: Option<String>,
    /// True for comments.
    pub comment: bool,
    /// Replies hidden under it (collapsed), or 0.
    pub hidden: usize,
    /// Replies under it (descendants).
    pub replies: usize,
}

/// A thread laid out at a width.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// The width it was laid out for (the whole screen width, gutter included).
    pub width: u16,
    /// The lines (without the gutter).
    pub lines: Vec<Line<'static>>,
    /// The items, in order.
    pub items: Vec<ItemSpan>,
}

impl Layout {
    /// The largest scroll offset for a viewport of `h` lines.
    pub fn max_scroll(&self, h: usize) -> usize {
        self.lines.len().saturating_sub(h)
    }

    /// The index of the item `uri`.
    pub fn index_of(&self, uri: &str) -> Option<usize> {
        self.items.iter().position(|i| i.uri == uri)
    }
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The number of descendants of each comment (pre-order with depth).
fn descendants(comments: &[CommentNode]) -> Vec<usize> {
    let mut out = vec![0usize; comments.len()];
    for (i, c) in comments.iter().enumerate() {
        let mut j = i + 1;
        while j < comments.len() && comments[j].depth > c.depth {
            j += 1;
        }
        out[i] = j - i - 1;
    }
    out
}

/// The verified handles of the thread's actors (for mentions in bodies).
pub fn handles(data: &ThreadData) -> BTreeMap<String, String> {
    data.view
        .comments
        .iter()
        .map(|c| &c.author)
        .chain(match &data.view.post {
            ThreadRoot::Post(p) => Some(&p.post.author),
            ThreadRoot::Missing { .. } => None,
        })
        .filter_map(|a| Some((a.did.clone(), a.handle.clone()?)))
        .collect()
}

/// The uris of the thread's items in order (the post, then every comment): two views with the
/// same sequence can replace each other without moving anything.
pub fn item_uris(data: &ThreadData) -> Vec<String> {
    let mut v = vec![root_uri(data)];
    v.extend(data.view.comments.iter().map(|c| c.uri.clone()));
    v
}

/// The thread's root post uri (or the missing one).
pub fn root_uri(data: &ThreadData) -> String {
    match &data.view.post {
        ThreadRoot::Post(p) => p.post.uri.clone(),
        ThreadRoot::Missing { missing } => missing.clone(),
    }
}

/// Lays out `data` for a screen `width` columns wide.
pub fn layout(data: &ThreadData, collapsed: &BTreeSet<String>, width: u16) -> Layout {
    let text_w = usize::from(width.saturating_sub(GUTTER)).max(20);
    let handles = handles(data);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut items = Vec::new();

    // the forum's banners (moderationUnavailable, newerRules)
    for b in banner_lines(&data.view.banners) {
        lines.push(line([(
            b,
            Style::default().fg(ratatui::style::Color::Yellow),
        )]));
    }

    // the post
    let start = lines.len();
    match &data.view.post {
        ThreadRoot::Post(p) => {
            lines.push(line([(post_title(&p.post), bold())]));
            let mut meta = post_meta(&p.post);
            if p.history.revisions > 0 {
                meta.push(count(p.history.revisions, "revision", "revisions"));
            }
            lines.push(line([(joined(meta), dim())]));
            if let Some(l) = &p.post.link {
                lines.push(line([
                    (lit("link: "), dim()),
                    (tt(&l.uri), Style::default()),
                ]));
            }
            if matches!(
                p.post.state,
                ItemState::Visible | ItemState::Collapsed { .. }
            ) && let Some(body) = data.bodies.get(&p.post.uri)
            {
                let body_lines = body.lines(text_w.min(MAX_BODY), 0, &handles);
                if !body_lines.is_empty() {
                    lines.push(Line::default());
                    lines.extend(body_lines);
                }
            }
        }
        ThreadRoot::Missing { missing } => {
            lines.push(line([(lit("[the post is not available]"), dim())]));
            lines.push(line([(tt(missing), dim())]));
        }
    }
    items.push(ItemSpan {
        uri: root_uri(data),
        start,
        end: lines.len(),
        parent: None,
        comment: false,
        hidden: 0,
        replies: data.view.comments.len(),
    });

    // the comments
    lines.push(Line::default());
    let n = u32::try_from(data.view.comments.len()).unwrap_or(u32::MAX);
    if n == 0 {
        lines.push(line([(lit("No comments yet."), dim())]));
    } else {
        lines.push(line([(count(n, "comment", "comments"), bold())]));
    }
    let desc = descendants(&data.view.comments);
    let min_depth = data
        .view
        .comments
        .iter()
        .map(|c| c.depth)
        .min()
        .unwrap_or(1);
    let max_indent = (text_w / 2).min(40);
    let mut skip_until_depth: Option<u32> = None;
    for (i, c) in data.view.comments.iter().enumerate() {
        if let Some(d) = skip_until_depth {
            if c.depth > d {
                continue;
            }
            skip_until_depth = None;
        }
        let folded = collapsed.contains(&c.uri) && desc[i] > 0;
        if folded {
            skip_until_depth = Some(c.depth);
        }
        let depth = usize::try_from(c.depth.saturating_sub(min_depth)).unwrap_or(0);
        let indent = (depth * 2).min(max_indent);
        lines.push(Line::default());
        let start = lines.len();
        lines.push(comment_header(c, indent, folded.then_some(desc[i])));
        if matches!(c.state, ItemState::Visible | ItemState::Collapsed { .. })
            && !c.missing
            && let Some(body) = data.bodies.get(&c.uri)
        {
            let w = text_w.saturating_sub(indent + 2).min(MAX_BODY);
            lines.extend(body.lines(w, indent + 2, &handles));
        }
        items.push(ItemSpan {
            uri: c.uri.clone(),
            start,
            end: lines.len(),
            parent: c.parent.clone(),
            comment: true,
            hidden: if folded { desc[i] } else { 0 },
            replies: desc[i],
        });
    }
    Layout {
        width,
        lines,
        items,
    }
}

fn comment_header(c: &CommentNode, indent: usize, folded: Option<usize>) -> Line<'static> {
    let mut parts: Vec<(TermText, Style)> = vec![(pad(indent), Style::default())];
    if c.missing {
        parts.push((lit("[missing comment]"), dim()));
        return line(parts);
    }
    parts.push((actor(&c.author), bold()));
    let mut meta = vec![TermText::empty(), tt(&time(c.created_at_ms))];
    meta.extend(score_parts(&c.score));
    if c.retracted {
        meta.push(lit("retracted"));
    }
    if c.orphaned {
        meta.push(lit("orphaned"));
    }
    if let Some(sl) = state_label(&c.state) {
        meta.push(sl);
    }
    meta.extend(badges(&c.badges));
    parts.push((joined(meta), dim()));
    if let Some(n) = folded {
        parts.push((lit("  "), Style::default()));
        parts.push((
            crate::text::cat([lit("[+"), tt(&n.to_string()), lit("]")]),
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    line(parts)
}

/// The thread screen's view position: the selected item and the first line shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    /// The selected item (index into [`Layout::items`]).
    pub selected: usize,
    /// The first line shown.
    pub scroll: usize,
}

impl Pos {
    /// Clamps the position to `l` and a viewport of `h` lines.
    pub fn clamp(&mut self, l: &Layout, h: usize) {
        self.selected = self.selected.min(l.items.len().saturating_sub(1));
        self.scroll = self.scroll.min(l.max_scroll(h));
    }

    /// Scrolls the least so that the selected item shows: all of it when it fits, else its
    /// first line (`from_end`: its last lines, when moving up into it).
    pub fn reveal(&mut self, l: &Layout, h: usize, from_end: bool) {
        let Some(it) = l.items.get(self.selected) else {
            return;
        };
        let tall = it.end - it.start > h;
        if it.start < self.scroll || it.end > self.scroll + h {
            self.scroll = if !tall {
                if it.start < self.scroll {
                    it.start
                } else {
                    it.end - h
                }
            } else if from_end {
                it.end - h
            } else {
                it.start
            };
        }
        self.scroll = self.scroll.min(l.max_scroll(h));
    }

    /// `j`: scroll through a tall item, else select the next one.
    pub fn down(&mut self, l: &Layout, h: usize) {
        let Some(it) = l.items.get(self.selected) else {
            return;
        };
        if it.end > self.scroll + h && self.scroll < l.max_scroll(h) {
            self.scroll += 1;
        } else if self.selected + 1 < l.items.len() {
            self.selected += 1;
            self.reveal(l, h, false);
        } else {
            self.scroll = l.max_scroll(h);
        }
    }

    /// `k`: scroll back through a tall item, else select the previous one.
    pub fn up(&mut self, l: &Layout, h: usize) {
        let Some(it) = l.items.get(self.selected) else {
            return;
        };
        if it.start < self.scroll {
            self.scroll -= 1;
        } else if self.selected > 0 {
            self.selected -= 1;
            self.reveal(l, h, true);
        } else {
            self.scroll = 0;
        }
    }

    /// `g`.
    pub fn top(&mut self) {
        *self = Self::default();
    }

    /// `G`.
    pub fn bottom(&mut self, l: &Layout, h: usize) {
        self.selected = l.items.len().saturating_sub(1);
        self.scroll = l.max_scroll(h);
        self.reveal(l, h, true);
    }

    /// `Ctrl-d`: half a page down; the selection follows if it scrolled out.
    pub fn half_down(&mut self, l: &Layout, h: usize) {
        let max = l.max_scroll(h);
        if self.scroll >= max {
            self.selected = l.items.len().saturating_sub(1);
            return;
        }
        self.scroll = (self.scroll + (h / 2).max(1)).min(max);
        self.follow(l, h);
    }

    /// `Ctrl-u`: half a page up; the selection follows if it scrolled out.
    pub fn half_up(&mut self, l: &Layout, h: usize) {
        if self.scroll == 0 {
            self.selected = 0;
            return;
        }
        self.scroll = self.scroll.saturating_sub((h / 2).max(1));
        self.follow(l, h);
    }

    /// After a scroll: if the selection is out of view, select the first item that shows
    /// (or the last one above the viewport's end when scrolling up).
    fn follow(&mut self, l: &Layout, h: usize) {
        let visible = |it: &ItemSpan| it.end > self.scroll && it.start < self.scroll + h;
        if l.items.get(self.selected).is_some_and(visible) {
            return;
        }
        if let Some(i) = l.items.iter().position(visible) {
            self.selected = i;
        }
    }

    /// `p`: the parent comment, or the post for a top-level comment.
    pub fn parent(&mut self, l: &Layout, h: usize) {
        let Some(it) = l.items.get(self.selected) else {
            return;
        };
        if !it.comment {
            return;
        }
        self.selected = it
            .parent
            .as_deref()
            .and_then(|p| l.index_of(p))
            .unwrap_or(0);
        self.reveal(l, h, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_of(spans: &[(usize, usize)]) -> Layout {
        let end = spans.last().map_or(0, |s| s.1);
        Layout {
            width: 80,
            lines: vec![Line::default(); end],
            items: spans
                .iter()
                .enumerate()
                .map(|(i, &(start, end))| ItemSpan {
                    uri: format!("u{i}"),
                    start,
                    end,
                    parent: (i > 1).then(|| "u1".to_owned()),
                    comment: i > 0,
                    hidden: 0,
                    replies: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn tall_items_scroll_before_moving_on() {
        // a 10-line post, then two 2-line comments, in a 4-line viewport
        let l = layout_of(&[(0, 10), (11, 13), (14, 16)]);
        let mut p = Pos::default();
        for _ in 0..6 {
            p.down(&l, 4);
        }
        assert_eq!(
            p,
            Pos {
                selected: 0,
                scroll: 6
            }
        );
        p.down(&l, 4);
        assert_eq!(p.selected, 1);
        assert_eq!(p.scroll, 9);
        p.down(&l, 4);
        assert_eq!(
            p,
            Pos {
                selected: 2,
                scroll: 12
            }
        );
        p.up(&l, 4);
        assert_eq!(p.selected, 1);
        p.up(&l, 4);
        assert_eq!(p.selected, 0);
        assert_eq!(p.scroll, 6, "the end of the tall post shows");
        p.up(&l, 4);
        assert_eq!(p.scroll, 5);
        p.top();
        assert_eq!(p, Pos::default());
        p.bottom(&l, 4);
        assert_eq!(
            p,
            Pos {
                selected: 2,
                scroll: 12
            }
        );
        p.parent(&l, 4);
        assert_eq!(p.selected, 1);
        p.parent(&l, 4);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn half_pages_move_the_selection_along() {
        let l = layout_of(&[(0, 2), (3, 5), (6, 8), (9, 11), (12, 14)]);
        let mut p = Pos::default();
        p.half_down(&l, 6);
        assert_eq!(
            p,
            Pos {
                selected: 1,
                scroll: 3
            }
        );
        p.half_down(&l, 6);
        assert_eq!(
            p,
            Pos {
                selected: 2,
                scroll: 6
            }
        );
        p.half_down(&l, 6);
        assert_eq!(
            p,
            Pos {
                selected: 3,
                scroll: 8
            }
        );
        p.half_down(&l, 6);
        assert_eq!(p.selected, 4);
        p.half_up(&l, 6);
        assert_eq!(p.scroll, 5);
        p.half_up(&l, 6);
        p.half_up(&l, 6);
        assert_eq!(
            p,
            Pos {
                selected: 0,
                scroll: 0
            }
        );
    }
}
