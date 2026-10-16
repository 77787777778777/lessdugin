// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The SPA's single HTML sink (threat-model.md SPA-2, SPA-3; lfm-1.md §8.5).
//
// - `LFM_CONFIG` is DOMPurify configured with exactly the renderer's allowlist (lfm-1.md §8.2
//   and the MathML filter of §7.3): the allowed tags and attributes, `ALLOW_DATA_ATTR: false`,
//   the URI regexp, and a hook that enforces per-element attributes, class tokens and value
//   grammars. `ALLOW_ARIA_ATTR: false` and `ADD_URI_SAFE_ATTR` keep it exact: without the
//   latter DOMPurify would test every non-URL attribute value (`dir="auto"`, `rel`, …) against
//   the URI regexp and drop it.
// - The Trusted Types policy `lessdugin-md` wraps `DOMPurify.sanitize(html, LFM_CONFIG)`. It is
//   created once, at startup, and is the only way rendered content reaches the DOM.
// - This module is the only place that assigns `innerHTML` (scripts/check-no-raw-href.mjs).

import DOMPurify, { type Config } from "dompurify";

const HTML_TAGS = [
  "div",
  "p",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "li",
  "ol",
  "blockquote",
  "ul",
  "table",
  "thead",
  "tbody",
  "tr",
  "figure",
  "pre",
  "em",
  "strong",
  "del",
  "hr",
  "br",
  "th",
  "td",
  "input",
  "code",
  "sup",
  "a",
  "details",
  "summary",
  "figcaption",
  "img",
  "span",
  "section",
] as const;

/** lfm-1.md §7.3 allowed MathML elements. */
const MATHML_TAGS = [
  "math",
  "mrow",
  "mi",
  "mn",
  "mo",
  "ms",
  "mtext",
  "mspace",
  "msub",
  "msup",
  "msubsup",
  "munder",
  "mover",
  "munderover",
  "mmultiscripts",
  "mprescripts",
  "none",
  "mfrac",
  "msqrt",
  "mroot",
  "mtable",
  "mtr",
  "mtd",
  "mpadded",
  "mphantom",
  "mstyle",
  "merror",
  "menclose",
] as const;

const HTML_ATTRS = [
  "class",
  "dir",
  "id",
  "start",
  "type",
  "checked",
  "disabled",
  "href",
  "data-at-uri",
  "data-did",
  "data-anchor",
  "rel",
  "data-cid",
  "alt",
  "width",
  "height",
  "data-tex",
] as const;

const MATH_BOOLEAN_ATTRS = [
  "displaystyle",
  "fence",
  "separator",
  "stretchy",
  "symmetric",
  "largeop",
  "movablelimits",
  "accent",
  "accentunder",
] as const;

const MATH_LENGTH_ATTRS = [
  "lspace",
  "rspace",
  "minsize",
  "maxsize",
  "linethickness",
  "width",
  "height",
  "depth",
  "voffset",
] as const;

const MATHML_ATTRS = [
  "display",
  ...MATH_BOOLEAN_ATTRS,
  "scriptlevel",
  "mathvariant",
  "form",
  ...MATH_LENGTH_ATTRS,
  "columnspan",
  "rowspan",
  "notation",
] as const;

const ALLOWED_ATTR = [...new Set<string>([...HTML_ATTRS, ...MATHML_ATTRS])];

/** `href` is the only URL-valued attribute; every other value is checked by the hook below. */
const URI_SAFE_ATTR = ALLOWED_ATTR.filter((a) => a !== "href");

export const LFM_URI_REGEXP = /^(?:https?:|mailto:|#\/at\/|#\/u\/)/;

export const LFM_CONFIG = {
  ALLOWED_TAGS: [...HTML_TAGS, ...MATHML_TAGS] as string[],
  ALLOWED_ATTR,
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: false,
  ALLOWED_URI_REGEXP: LFM_URI_REGEXP,
  ADD_URI_SAFE_ATTR: URI_SAFE_ATTR,
} satisfies Config;

// ------------------------------------------------------------------------- value grammars

type Check = (value: string, el: Element) => boolean;

const oneOf =
  (...values: string[]): Check =>
  (v) =>
    values.includes(v);
const AUTO = oneOf("auto");
const EMPTY = oneOf("");
const ANY: Check = () => true;
const CID: Check = (v) => /^b[a-z2-7]{1,127}$/.test(v);
const DIMENSION: Check = (v) => /^[1-9][0-9]{0,7}$/.test(v);
const HEADING: Record<string, Check> = { id: (v) => /^md-h-[\p{L}\p{N}_-]*$/u.test(v), dir: AUTO };

const HTML_RULES: Record<string, Record<string, Check>> = {
  div: {
    class: oneOf(
      "lfm",
      "alert-note",
      "alert-tip",
      "alert-important",
      "alert-warning",
      "alert-caution",
      "gallery",
    ),
    dir: (v, el) => v === "auto" && el.getAttribute("class") === "lfm",
  },
  p: { class: oneOf("notice"), dir: AUTO },
  h1: HEADING,
  h2: HEADING,
  h3: HEADING,
  h4: HEADING,
  h5: HEADING,
  h6: HEADING,
  li: { id: (v) => /^md-fn-[1-9][0-9]*$/.test(v), dir: AUTO },
  ol: { start: (v) => /^(0|[1-9][0-9]{0,9})$/.test(v) },
  th: { class: oneOf("ta-l", "ta-c", "ta-r"), dir: AUTO },
  td: { class: oneOf("ta-l", "ta-c", "ta-r"), dir: AUTO },
  input: { type: oneOf("checkbox"), checked: EMPTY, disabled: EMPTY },
  code: {
    class: (v, el) =>
      v === "math-error" ||
      (/^language-[a-z0-9_+-]{1,32}$/.test(v) && el.parentElement?.localName === "pre"),
  },
  sup: { id: (v) => /^md-fnref-[1-9][0-9]*-[1-9][0-9]*$/.test(v) },
  a: {
    href: (v) => /^(https?:\/\/|mailto:|#\/at\/|#\/u\/)/.test(v),
    "data-at-uri": (v) => /^at:\/\/did:(plc|web):[^\s"'<>/]+\/[^\s"'<>/]+\/[^\s"'<>/]+$/.test(v),
    "data-did": (v) => /^did:(plc:[a-z2-7]{24}|web:[a-z0-9.-]+)$/.test(v),
    "data-anchor": (v) => /^[\p{L}\p{N}_-]{1,512}$/u.test(v),
    class: oneOf("at-link", "mention", "ext-image", "fn-back"),
    rel: oneOf("nofollow ugc noopener noreferrer"),
  },
  details: { class: oneOf("spoiler") },
  summary: { dir: AUTO },
  figcaption: { dir: AUTO },
  img: { "data-cid": CID, alt: ANY, width: DIMENSION, height: DIMENSION },
  span: {
    class: oneOf("media", "media-missing", "link-host"),
    "data-cid": (v, el) => CID(v, el) && el.getAttribute("class") === "media",
    "data-tex": (v, el) => (v === "inline" || v === "block") && !el.hasAttribute("class"),
  },
  section: { class: oneOf("footnotes") },
};

const isMathBoolean = (v: string): boolean => v === "true" || v === "false";
const isMathLength = (v: string): boolean =>
  v.length <= 32 && /^[+-]?[0-9]*\.?[0-9]+([a-z]{1,3}|%)?$/.test(v);

function mathAttrAllowed(tag: string, name: string, value: string): boolean {
  if (name === "display") return tag === "math" && value === "block";
  if ((MATH_BOOLEAN_ATTRS as readonly string[]).includes(name)) return isMathBoolean(value);
  if (name === "scriptlevel") return /^[+-]?[0-9]{1,2}$/.test(value);
  if (name === "mathvariant") return value === "normal";
  if (name === "form") return tag === "mo" && ["prefix", "infix", "postfix"].includes(value);
  if ((MATH_LENGTH_ATTRS as readonly string[]).includes(name)) return isMathLength(value);
  if (name === "columnspan" || name === "rowspan")
    return tag === "mtd" && /^[1-9][0-9]{0,2}$/.test(value);
  if (name === "notation") return tag === "menclose" && /^[a-z ]{1,128}$/.test(value);
  return false;
}

const MATHML_NS = "http://www.w3.org/1998/Math/MathML";

/** True if `name="value"` is allowed on `el` by lfm-1.md §8.2 / §7.3. */
export function attributeAllowed(el: Element, name: string, value: string): boolean {
  const tag = el.localName;
  if (el.namespaceURI === MATHML_NS || (MATHML_TAGS as readonly string[]).includes(tag)) {
    return mathAttrAllowed(tag, name, value);
  }
  return HTML_RULES[tag]?.[name]?.(value, el) ?? false;
}

let hooksInstalled = false;

function installHooks(): void {
  if (hooksInstalled) return;
  hooksInstalled = true;
  DOMPurify.addHook("uponSanitizeAttribute", (node, data) => {
    if (!attributeAllowed(node as Element, data.attrName, data.attrValue)) data.keepAttr = false;
  });
}

/** `DOMPurify.sanitize(html, LFM_CONFIG)` as a string. */
export function sanitizeLfm(html: string): string {
  installHooks();
  return DOMPurify.sanitize(html, {
    ...LFM_CONFIG,
    RETURN_TRUSTED_TYPE: false,
  }) as unknown as string;
}

/** `null` when the sanitiser leaves `html` unchanged (the renderer's contract, lfm-1.md §8.3),
 * otherwise the sanitised output. */
export function sanitizerMismatch(html: string): string | null {
  const out = sanitizeLfm(html);
  return out === html ? null : out;
}

// ------------------------------------------------------------------------- Trusted Types

interface TrustedHtmlPolicy {
  createHTML(input: string): unknown;
}

interface TrustedTypesFactory {
  createPolicy(name: string, rules: { createHTML: (input: string) => string }): TrustedHtmlPolicy;
}

const POLICY_KEY = Symbol.for("lessdugin.trusted-types.lessdugin-md");
type PolicyHolder = { [POLICY_KEY]?: TrustedHtmlPolicy | null };

/** Creates the `lessdugin-md` policy once (call at startup). `null` without Trusted Types. */
export function lessduginPolicy(): TrustedHtmlPolicy | null {
  const holder = globalThis as unknown as PolicyHolder;
  const existing = holder[POLICY_KEY];
  if (existing !== undefined) return existing;
  const tt = (globalThis as unknown as { trustedTypes?: TrustedTypesFactory }).trustedTypes;
  const policy = tt?.createPolicy
    ? tt.createPolicy("lessdugin-md", { createHTML: (html) => sanitizeLfm(html) })
    : null;
  holder[POLICY_KEY] = policy;
  return policy;
}

/** Replaces `el`'s children with sanitised `html`, through the `lessdugin-md` policy when
 * Trusted Types are available. */
export function setSanitizedHtml(el: Element, html: string): void {
  const policy = lessduginPolicy();
  el.innerHTML = policy ? (policy.createHTML(html) as string) : sanitizeLfm(html);
}
