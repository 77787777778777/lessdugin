// SPDX-License-Identifier: AGPL-3.0-or-later
//! The limit stage (`docs/spec/lfm-1.md` §8.4): S6 → SL. Output DOM depth (not counting the
//! root `div`) never exceeds 64; deeper blocks are flattened into paragraphs and deeper inline
//! tags are dropped. Both writers consume SL, so HTML and `StyledDoc` flatten identically.

use crate::event::{Ev, LinkRes, Tag, TagKind, match_indices, push_text};

/// The maximum output depth.
pub(crate) const MAX_DEPTH: usize = 64;

/// The depth at which footnote-definition content starts (`section`, `ol`, `li`).
pub(crate) const FOOTNOTE_BASE: usize = 3;

fn cost(tag: &Tag) -> usize {
    match tag {
        Tag::CodeBlock(_) | Tag::Table(_) => 2,
        // A link dropped by the URL policy, and images (leaves), open no element.
        Tag::Link(d) if d.res == LinkRes::Dropped => 0,
        Tag::Image(_) | Tag::FootnoteDef { .. } => 0,
        _ => 1,
    }
}

fn reserve(tag: &Tag) -> usize {
    match tag {
        Tag::List(_) | Tag::Figure(_) | Tag::Spoiler | Tag::Details => 1,
        Tag::Table(_) => 2,
        _ => 0,
    }
}

/// Run the limit stage over one flow (the body at depth 0, or a footnote at
/// [`FOOTNOTE_BASE`]). Returns SL and whether anything was flattened.
pub(crate) fn limit(evs: Vec<Ev>, base: usize) -> (Vec<Ev>, bool) {
    let m = match_indices(&evs);
    let mut out = Vec::with_capacity(evs.len());
    let mut limited = false;
    // Per open admitted/dropped Start: the depth it added (None = dropped, End skipped).
    let mut open: Vec<Option<usize>> = Vec::new();
    let mut d = base;
    let mut i = 0usize;
    while i < evs.len() {
        match &evs[i] {
            Ev::Start(tag) => {
                if let Tag::Image(_) = tag {
                    // Leaf: copy the whole subtree unchanged (writers use the computed alt).
                    let end = m[i].min(evs.len() - 1);
                    out.extend_from_slice(&evs[i..=end]);
                    i = end + 1;
                    continue;
                }
                let c = cost(tag);
                let k = tag.kind();
                if d + c + reserve(tag) <= MAX_DEPTH {
                    open.push(Some(c));
                    d += c;
                    out.push(evs[i].clone());
                } else if matches!(
                    k,
                    TagKind::Emphasis | TagKind::Strong | TagKind::Strikethrough | TagKind::Link
                ) {
                    open.push(None);
                } else if matches!(
                    k,
                    TagKind::Item
                        | TagKind::TableHead
                        | TagKind::TableRow
                        | TagKind::TableCell
                        | TagKind::Summary
                ) {
                    open.push(Some(c));
                    d += c;
                    out.push(evs[i].clone());
                } else {
                    limited = true;
                    let end = m[i].min(evs.len() - 1);
                    let flat = flatten(&evs[i..=end]);
                    if !flat.is_empty() {
                        if d < MAX_DEPTH {
                            out.push(Ev::Start(Tag::Paragraph));
                            out.extend(flat);
                            out.push(Ev::End(TagKind::Paragraph));
                        } else {
                            out.extend(flat);
                        }
                    }
                    i = end + 1;
                    continue;
                }
            }
            Ev::End(_) => match open.pop() {
                Some(Some(c)) => {
                    d -= c;
                    out.push(evs[i].clone());
                }
                Some(None) => {}
                None => out.push(evs[i].clone()),
            },
            e => out.push(e.clone()),
        }
        i += 1;
    }
    (out, limited)
}

/// `flat(subtree)` (§8.4): text, literal and code payloads as `Text`, math as its TeX, image
/// alts as `Text`; `SoftBreak` at block boundaries and for soft and hard breaks, never
/// repeated and never at either end; footnote references and task markers dropped.
fn flatten(sub: &[Ev]) -> Vec<Ev> {
    fn brk(out: &mut Vec<Ev>) {
        if !out.is_empty() && !matches!(out.last(), Some(Ev::SoftBreak)) {
            out.push(Ev::SoftBreak);
        }
    }
    let mut out: Vec<Ev> = Vec::new();
    let mut i = 0usize;
    let mut image_depth = 0usize;
    while i < sub.len() {
        let e = &sub[i];
        if image_depth > 0 {
            match e {
                Ev::Start(Tag::Image(_)) => image_depth += 1,
                Ev::End(TagKind::Image) => image_depth -= 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        match e {
            Ev::Text(t) | Ev::Literal(t) | Ev::Code(t) | Ev::InlineMath(t) | Ev::DisplayMath(t) => {
                push_text(&mut out, t)
            }
            Ev::Start(Tag::Image(d)) => {
                push_text(&mut out, &d.alt);
                image_depth = 1;
            }
            Ev::Start(t) if t.kind().is_block() => brk(&mut out),
            Ev::End(k) if k.is_block() => brk(&mut out),
            Ev::SoftBreak | Ev::HardBreak | Ev::Rule => brk(&mut out),
            _ => {}
        }
        i += 1;
    }
    while matches!(out.last(), Some(Ev::SoftBreak)) {
        out.pop();
    }
    out
}
