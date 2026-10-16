// SPDX-License-Identifier: AGPL-3.0-or-later
// A TypeScript port of `safe_href` (lfm-1.md §5.1, §5.2) for the mock engine. The real engine
// exports the Rust implementation through wasm; this one exists so the mock behaves alike.

import type { Href, HrefContext } from "../types.ts";
import { utf8Length } from "./util.ts";

const BIDI = new Set([
  0x061c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068, 0x2069,
]);

function forbiddenCodePoint(c: number): boolean {
  return (
    c <= 0x20 ||
    c === 0x7f ||
    (c >= 0x80 && c <= 0x9f) ||
    BIDI.has(c) ||
    c === 0x2028 ||
    c === 0x2029 ||
    (c >= 0x200b && c <= 0x200d) ||
    c === 0x2060 ||
    c === 0xfeff ||
    c === 0x5c // backslash
  );
}

const PCT_KEEP = /[A-Za-z0-9\-._~:/?#[\]@!$&'()*+,;=%]/;

/** lfm-1.md §5.1 `pct`. */
function pct(x: string): string {
  let out = "";
  for (const ch of x) {
    if (ch.length === 1 && PCT_KEEP.test(ch)) {
      out += ch;
      continue;
    }
    for (const b of new TextEncoder().encode(ch)) {
      out += `%${b.toString(16).toUpperCase().padStart(2, "0")}`;
    }
  }
  return out;
}

export const DID_PLC = /^did:plc:[a-z2-7]{24}$/;
export const DID_WEB = /^did:web:[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/;
const NSID =
  /^[a-zA-Z]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+$/;
const RKEY = /^[A-Za-z0-9._:~-]{1,512}$/;
const HANDLE = /^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]([a-z0-9-]{0,61}[a-z0-9])?$/;

export function isDid(s: string): boolean {
  return DID_PLC.test(s) || DID_WEB.test(s);
}

export function isHandle(s: string): boolean {
  return HANDLE.test(s.toLowerCase()) && s.length <= 253;
}

export function isNsid(s: string): boolean {
  return NSID.test(s);
}

export function isRkey(s: string): boolean {
  return RKEY.test(s) && s !== "." && s !== "..";
}

/** lfm-1.md §5.2 `parse_at_link` (rest = the text after `at:`). */
export function parseAtLink(rest: string, ctx: HrefContext): Href | null {
  if (!rest.startsWith("//")) return null;
  const body = rest.slice(2);
  if (body.includes("?") || body.includes("#")) return null;
  const segs = body.split("/");
  const first = segs[0] ?? "";
  if (first.startsWith("did:")) {
    if (!isDid(first)) return null;
    if (segs.length === 1) return { kind: "profile", did: first };
    if (segs.length === 3) {
      const nsid = segs[1] ?? "";
      const rkey = segs[2] ?? "";
      if (!isNsid(nsid) || !isRkey(rkey)) return null;
      return { kind: "record", uri: `at://${first}/${nsid}/${rkey}` };
    }
    return null;
  }
  if (segs.length === 1 && ctx === "field" && isHandle(first)) {
    return { kind: "profileHandle", handle: first.toLowerCase() };
  }
  return null;
}

/** lfm-1.md §5.1 `safe_href`. */
export function safeHref(raw: string, ctx: HrefContext): Href | null {
  if (raw.length === 0 || utf8Length(raw) > 3000) return null;
  for (const ch of raw) {
    if (forbiddenCodePoint(ch.codePointAt(0) ?? 0)) return null;
  }
  if (raw.startsWith("#")) {
    if (ctx !== "markdown") return null;
    let v: string;
    try {
      v = decodeURIComponent(raw.slice(1));
    } catch {
      return null;
    }
    v = v.toLowerCase();
    if (v.length === 0 || utf8Length(v) > 512 || !/^[\p{L}\p{N}_-]+$/u.test(v)) return null;
    return { kind: "anchor", id: v };
  }
  const m = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(raw);
  if (!m?.[1]) return null;
  const scheme = m[1].toLowerCase();
  const rest = raw.slice(m[0].length);
  if (scheme === "https" || scheme === "http") {
    if (!rest.startsWith("//")) return null;
    const after = rest.slice(2);
    const end = after.search(/[/?#]/);
    const authority = end < 0 ? after : after.slice(0, end);
    const tail = end < 0 ? "" : after.slice(end);
    if (authority.includes("@")) return null;
    let host: string;
    let port: string | undefined;
    if (authority.startsWith("[")) {
      const close = authority.indexOf("]");
      if (close < 0) return null;
      host = authority.slice(0, close + 1).toLowerCase();
      if (!/^\[[0-9a-f:.]+\]$/.test(host)) return null;
      const afterHost = authority.slice(close + 1);
      if (afterHost !== "") {
        if (!afterHost.startsWith(":")) return null;
        port = afterHost.slice(1);
      }
    } else {
      const colon = authority.indexOf(":");
      host = (colon < 0 ? authority : authority.slice(0, colon)).toLowerCase();
      if (colon >= 0) port = authority.slice(colon + 1);
      if (!/^[a-z0-9._-]+$/.test(host) || !/[^.]/.test(host)) return null;
    }
    if (port !== undefined && (!/^[0-9]{1,5}$/.test(port) || Number(port) > 65535)) return null;
    const url = `${scheme}://${host}${port !== undefined ? `:${port}` : ""}${pct(tail)}`;
    return { kind: "external", url, host };
  }
  if (scheme === "mailto") {
    if (rest.length === 0 || rest.startsWith("/")) return null;
    const q = rest.indexOf("?");
    const addr = q < 0 ? rest : rest.slice(0, q);
    const at = addr.lastIndexOf("@");
    const host = at < 0 ? "" : addr.slice(at + 1).toLowerCase();
    return { kind: "mailto", url: `mailto:${pct(rest)}`, host };
  }
  if (scheme === "at") return parseAtLink(rest, ctx);
  return null;
}
