// SPDX-License-Identifier: AGPL-3.0-or-later
// A deliberately small markdown renderer for the mock engine. It covers paragraphs, ATX
// headings, block quotes, GFM alerts, `[!spoiler]`, flat lists, fenced code, and the inline
// constructs `code`, **strong**, *em*/_em_, ~~del~~ and [links](…) through `safeHref`. Its output
// stays inside the LFM/1 allowlist (lfm-1.md §8.2) and uses the same serialisation (§8.3), so the
// SPA's sanitiser round-trips it unchanged. Everything else is shown as escaped text. The real
// renderer (`crates/markdown`) replaces it.

import type { Did, RenderedBody } from "../types.ts";
import { safeHref } from "./safe-href.ts";
import {
  escapeAttr,
  escapeText,
  excerptOf,
  htmlToText,
  MOCK_RENDERER_VERSION,
  normalizeP,
  readMinutes,
  shortDid,
  tocOf,
  wordCount,
} from "./util.ts";

const EXT_REL = "nofollow ugc noopener noreferrer";
const ALERTS = new Set(["note", "tip", "important", "warning", "caution"]);

/** lfm-1.md §4.6 slug, without the dedup. */
function slug(text: string): string {
  return Array.from(text.normalize("NFC").toLowerCase())
    .filter((c) => c === " " || c === "-" || c === "_" || /[\p{L}\p{N}]/u.test(c))
    .join("")
    .replace(/ /g, "-");
}

interface Ctx {
  seen: Map<string, number>;
  used: Set<string>;
  mentions: Did[];
}

function headingId(ctx: Ctx, text: string): string {
  const s = slug(text);
  let r = s;
  while (ctx.used.has(r)) {
    const n = ctx.seen.get(s) ?? 0;
    ctx.seen.set(s, n + 1);
    r = `${s}-${n + 1}`;
  }
  ctx.used.add(r);
  return `md-h-${r}`;
}

/** lfm-1.md §5.5 host-like test. */
function hostLike(text: string, host: string): boolean {
  let t = text.normalize("NFC").trim().toLowerCase();
  if (t.startsWith("@")) t = t.slice(1);
  const at = t.lastIndexOf("@");
  if (at >= 0) t = t.slice(at + 1);
  t = t.replace(/^(https?:\/\/|mailto:)/, "").replace(/^www\./, "");
  const m = /^[a-z0-9_-]+(\.[a-z0-9_-]+)+(?=$|[/:?#])/.exec(t);
  if (!m) return false;
  return m[0] !== host.replace(/^www\./, "");
}

function inline(src: string, ctx: Ctx, inLink = false): string {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const rest = src.slice(i);
    let m: RegExpExecArray | null = /^`([^`]+)`/.exec(rest);
    if (m) {
      out += `<code>${escapeText(m[1] ?? "")}</code>`;
      i += m[0].length;
      continue;
    }
    m = inLink ? null : /^\[([^\]]+)\]\(([^)\s]+)\)/.exec(rest);
    if (m) {
      out += link(m[1] ?? "", m[2] ?? "", ctx);
      i += m[0].length;
      continue;
    }
    m = /^\*\*(\S(?:[^*]*\S)?)\*\*/.exec(rest);
    if (m) {
      out += `<strong>${inline(m[1] ?? "", ctx, inLink)}</strong>`;
      i += m[0].length;
      continue;
    }
    m = /^~~(\S(?:[^~]*\S)?)~~/.exec(rest);
    if (m) {
      out += `<del>${inline(m[1] ?? "", ctx, inLink)}</del>`;
      i += m[0].length;
      continue;
    }
    m = /^\*(\S(?:[^*]*\S)?)\*/.exec(rest) ?? /^_(\S(?:[^_]*\S)?)_(?![\p{L}\p{N}])/u.exec(rest);
    if (m) {
      out += `<em>${inline(m[1] ?? "", ctx, inLink)}</em>`;
      i += m[0].length;
      continue;
    }
    const ch = String.fromCodePoint(src.codePointAt(i) ?? 0);
    out += escapeText(ch);
    i += ch.length;
  }
  return out;
}

function link(text: string, dest: string, ctx: Ctx): string {
  const href = safeHref(dest, "markdown");
  const inner = inline(text, ctx, true);
  if (!href) return inner;
  switch (href.kind) {
    case "external":
    case "mailto": {
      const host = hostLike(text, href.host)
        ? `<span class="link-host">${escapeText(href.host)}</span>`
        : "";
      return `<a href="${escapeAttr(href.url)}" rel="${EXT_REL}">${inner}${host}</a>`;
    }
    case "record": {
      const u = href.uri.slice("at://".length);
      return `<a href="#/at/${escapeAttr(u)}" data-at-uri="${escapeAttr(href.uri)}" class="at-link">${inner}</a>`;
    }
    case "profile": {
      if (text.startsWith("@")) {
        if (!ctx.mentions.includes(href.did)) ctx.mentions.push(href.did);
        return `<a href="#/u/${escapeAttr(href.did)}" data-did="${escapeAttr(href.did)}" class="mention">@${escapeText(shortDid(href.did))}</a>`;
      }
      return `<a href="#/u/${escapeAttr(href.did)}" data-did="${escapeAttr(href.did)}" class="at-link">${inner}</a>`;
    }
    case "anchor":
      return `<a data-anchor="${escapeAttr(href.id)}">${inner}</a>`;
    default:
      return inner;
  }
}

function paragraph(lines: string[], ctx: Ctx): string {
  return `<p dir="auto">${inline(lines.join("\n"), ctx)}</p>\n`;
}

function blocks(lines: string[], ctx: Ctx): string {
  let out = "";
  let i = 0;
  let para: string[] = [];
  const flush = (): void => {
    if (para.length > 0) out += paragraph(para, ctx);
    para = [];
  };
  while (i < lines.length) {
    const line = lines[i] ?? "";
    const fence = /^```\s*([^\s`]*)\s*$/.exec(line);
    if (fence) {
      flush();
      const lang = (fence[1] ?? "").toLowerCase();
      const code: string[] = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i] ?? "")) code.push(lines[i++] ?? "");
      i++;
      const cls = /^[a-z0-9_+-]{1,32}$/.test(lang) ? ` class="language-${lang}"` : "";
      const text = code.length > 0 ? `${code.join("\n")}\n` : "";
      out += `<pre><code${cls}>${escapeText(text)}</code></pre>\n`;
      continue;
    }
    const heading = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading) {
      flush();
      const level = (heading[1] ?? "#").length;
      const content = heading[2] ?? "";
      const id = headingId(ctx, normalizeP(content.replace(/[*_`~]/g, "")));
      out += `<h${level} id="${escapeAttr(id)}" dir="auto">${inline(content, ctx)}</h${level}>\n`;
      i++;
      continue;
    }
    if (/^>\s?/.test(line)) {
      flush();
      const quoted: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i] ?? "")) {
        quoted.push((lines[i] ?? "").replace(/^>\s?/, ""));
        i++;
      }
      const marker = /^\[!([A-Za-z]+)\]\s*$/.exec(quoted[0] ?? "");
      const kind = marker?.[1]?.toLowerCase();
      if (kind && ALERTS.has(kind)) {
        out += `<div class="alert-${kind}">\n${blocks(quoted.slice(1), ctx)}</div>\n`;
      } else if (kind === "spoiler") {
        out += `<details class="spoiler">\n<summary dir="auto">Spoiler</summary>\n${blocks(quoted.slice(1), ctx)}</details>\n`;
      } else {
        out += `<blockquote>\n${blocks(quoted, ctx)}</blockquote>\n`;
      }
      continue;
    }
    const bullet = /^\s*[-*+]\s+(.*)$/.exec(line);
    const ordered = /^\s*(\d{1,9})[.)]\s+(.*)$/.exec(line);
    if (bullet || ordered) {
      flush();
      const isOrdered = !bullet;
      const start = ordered ? Number(ordered[1]) : 1;
      const items: string[] = [];
      while (i < lines.length) {
        const l = lines[i] ?? "";
        const b = isOrdered ? /^\s*\d{1,9}[.)]\s+(.*)$/.exec(l) : /^\s*[-*+]\s+(.*)$/.exec(l);
        if (!b) break;
        items.push(b[1] ?? "");
        i++;
      }
      const open = isOrdered ? (start === 1 ? "<ol>" : `<ol start="${start}">`) : "<ul>";
      out += `${open}\n`;
      for (const item of items) {
        const task = /^\[([ xX])\]\s(.*)$/.exec(item);
        if (task) {
          const box =
            task[1] === " "
              ? '<input type="checkbox" disabled="">'
              : '<input type="checkbox" checked="" disabled="">';
          out += `<li dir="auto">${box} ${inline(task[2] ?? "", ctx)}</li>\n`;
        } else {
          out += `<li dir="auto">${inline(item, ctx)}</li>\n`;
        }
      }
      out += isOrdered ? "</ol>\n" : "</ul>\n";
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      out += "<hr>\n";
      i++;
      continue;
    }
    if (line.trim() === "") {
      flush();
      i++;
      continue;
    }
    para.push(line);
    i++;
  }
  flush();
  return out;
}

export interface MockRendered {
  body: RenderedBody;
  mentions: Did[];
}

/** Renders markdown with the mock renderer. */
export function renderMarkdown(src: string, notices: string[] = []): MockRendered {
  const ctx: Ctx = { seen: new Map(), used: new Set(), mentions: [] };
  const normalised = src.replace(/\r\n?/g, "\n");
  const html = `<div class="lfm" dir="auto">\n${blocks(normalised.split("\n"), ctx)}</div>\n`;
  const text = htmlToText(html);
  const words = wordCount(text);
  return {
    body: {
      html,
      text,
      toc: tocOf(html),
      wordCount: words,
      readMinutes: readMinutes(words),
      media: [],
      excerpt: excerptOf(htmlToText(html, { omitSpoilers: true })),
      notices,
      rendererVersion: MOCK_RENDERER_VERSION,
    },
    mentions: ctx.mentions,
  };
}
