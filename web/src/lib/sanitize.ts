// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The SPA's single HTML sink (threat-model.md SPA-2, SPA-3; lfm-1.md §8.5).
//
// - `LFM_CONFIG` is DOMPurify configured with exactly the renderer's allowlist (lfm-1.md §8.2
//   and the MathML normaliser of §7.3, whose `class` tokens and `mathcolor` replace math-core's
//   inline styles): the allowed tags and attributes, `ALLOW_DATA_ATTR: false`,
//   the URI regexp, and hooks that enforce per-element attributes, class tokens, value grammars
//   and whole-element shapes. `ALLOW_ARIA_ATTR: false` and `ADD_URI_SAFE_ATTR` keep it exact:
//   without the latter DOMPurify would test every non-URL attribute value (`dir="auto"`, `rel`,
//   …) against the URI regexp and drop it.
// - The sanitiser is a dedicated DOMPurify instance, configured once with `setConfig`: its hooks
//   and configuration never leak into (or from) any other user of the shared default instance.
// - The Trusted Types policy `lessdugin-md` wraps `sanitize(html)`. It is created once, at
//   startup, and is the only way rendered content reaches the DOM.
// - This module is the only place that assigns `innerHTML` (scripts/check-no-raw-href.mjs).
//
// `SAFE_FOR_XML: false`. DOMPurify's SAFE_FOR_XML mode drops every attribute whose decoded value
// contains `</script`, `</style`, `-->` and similar, and every comment or raw-text element whose
// text looks like markup. It exists for output that may be re-parsed as XML or inside a raw-text
// context. Here the output is only ever assigned to `innerHTML` of an element in an HTML
// document (`setSanitizedHtml`), and the allowlist has no raw-text element (no `style`,
// `script`, `textarea`, `title`, `noscript`, `xmp`, `iframe`, `noembed`, `noframes`), no
// comments, and none of the declarative-partial-update attributes it also polices (`for`,
// `patchsrc`; dropped by the attribute hook). With it on, an `img alt` that merely contains the
// text `</script` (the `xss/html.json#attribute-breakout-media-alt` vector) loses its `alt`, so
// the sanitiser would change conforming renderer output, which lfm-1.md §8.3 forbids.

import DOMPurify, { type Config, type DOMPurify as Purifier } from "dompurify";
import { isDid, isRecordUri } from "./uri.ts";

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
  "class",
  "mathcolor",
] as const;

/**
 * lfm-1.md §7.3 MathML class tokens: the `mc-*` classes of the style normaliser (ADR 0002
 * decision 2; styled by `crates/markdown/assets/mathml.css`) and math-core's `menclose-*`
 * polyfill classes (`mathmlfixes.css`). The same list as `lessdugin_markdown::math::MATHML_CLASSES`.
 */
export const MATHML_CLASSES: ReadonlySet<string> = new Set([
  "mc-al",
  "mc-ar",
  "mc-bl-dashed",
  "mc-bl-solid",
  "mc-br-dashed",
  "mc-br-solid",
  "mc-bt-dashed",
  "mc-bt-solid",
  "mc-cramped",
  "mc-fs100",
  "mc-fs120",
  "mc-fs140",
  "mc-fs170",
  "mc-fs200",
  "mc-fs250",
  "mc-fs50",
  "mc-fs60",
  "mc-fs70",
  "mc-fs80",
  "mc-fs90",
  "mc-negmed",
  "mc-negthick",
  "mc-negthin",
  "mc-pl0",
  "mc-pl1",
  "mc-pl3mu",
  "mc-pl4mu",
  "mc-pl5mu",
  "mc-pr0",
  "mc-pr3mu",
  "mc-pr4mu",
  "mc-pr5mu",
  "mc-pt3mu",
  "mc-px01",
  "mc-py0",
  "mc-tb",
  "mc-ti",
  "mc-trm",
  "mc-ts",
  "mc-tsc",
  "mc-tsf",
  "mc-tt",
  "mc-tu",
  "mc-w100",
  "mc-w50",
  "mc-w7-5",
  "menclose-downdiagonalstrike",
  "menclose-phasorangle",
  "menclose-updiagonalstrike",
]);

const ALLOWED_ATTR = [...new Set<string>([...HTML_ATTRS, ...MATHML_ATTRS])];

/** `href` is the only URL-valued attribute; every other value is checked by the hooks below. */
const URI_SAFE_ATTR = ALLOWED_ATTR.filter((a) => a !== "href");

export const LFM_URI_REGEXP = /^(?:https?:|mailto:|#\/at\/|#\/u\/)/;

export const LFM_CONFIG = {
  ALLOWED_TAGS: [...HTML_TAGS, ...MATHML_TAGS] as string[],
  ALLOWED_ATTR,
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: false,
  ALLOWED_URI_REGEXP: LFM_URI_REGEXP,
  ADD_URI_SAFE_ATTR: URI_SAFE_ATTR,
  SAFE_FOR_XML: false,
  RETURN_TRUSTED_TYPE: false,
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
const EXTERNAL_HREF = /^(?:https?:\/\/|mailto:)/;
const WEB_HREF = /^https?:\/\//;

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
    "data-at-uri": isRecordUri,
    "data-did": isDid,
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

/** One or more §7.3 class tokens, ascending (code-unit order), unique, one space apart. */
function isMathClassList(v: string): boolean {
  const tokens = v.split(" ");
  return tokens.every(
    (t, i) => MATHML_CLASSES.has(t) && (i === 0 || (tokens[i - 1] as string) < t),
  );
}

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
  if (name === "class") return isMathClassList(value);
  if (name === "mathcolor") return /^#[0-9a-f]{6}$/.test(value);
  return false;
}

const MATHML_NS = "http://www.w3.org/1998/Math/MathML";
const HTML_NS = "http://www.w3.org/1999/xhtml";

/** True if `name="value"` is allowed on `el` by lfm-1.md §8.2 / §7.3. */
export function attributeAllowed(el: Element, name: string, value: string): boolean {
  const tag = el.localName;
  if (el.namespaceURI === MATHML_NS || (MATHML_TAGS as readonly string[]).includes(tag)) {
    return mathAttrAllowed(tag, name, value);
  }
  return HTML_RULES[tag]?.[name]?.(value, el) ?? false;
}

// ------------------------------------------------------------------------- element shapes

/** The element's attribute names, sorted and comma-joined (the "shape key"). */
function shapeKey(el: Element): string {
  return el.getAttributeNames().sort().join(",");
}

/**
 * `a` shapes of lfm-1.md §8.1, checked on the attributes that survived the value grammars:
 * external (`href` + `rel`, optionally `class="ext-image"`), record link (`href` exactly
 * `#/at/` + the at-uri without `at://`), profile link or mention (`href` exactly `#/u/` + the
 * DID), and anchor links (`data-anchor`, optionally `class="fn-back"`).
 */
export function linkShapeValid(el: Element): boolean {
  const href = el.getAttribute("href") ?? "";
  const cls = el.getAttribute("class");
  switch (shapeKey(el)) {
    case "data-anchor":
      return true;
    case "class,data-anchor":
      return cls === "fn-back";
    case "href,rel":
      return EXTERNAL_HREF.test(href);
    case "class,href,rel":
      return cls === "ext-image" && WEB_HREF.test(href);
    case "class,data-at-uri,href": {
      const uri = el.getAttribute("data-at-uri") ?? "";
      return cls === "at-link" && uri.startsWith("at://") && href === `#/at/${uri.slice(5)}`;
    }
    case "class,data-did,href":
      return (
        (cls === "at-link" || cls === "mention") && href === `#/u/${el.getAttribute("data-did")}`
      );
    default:
      return false;
  }
}

const SHAPES: Record<string, (el: Element) => boolean> = {
  a: linkShapeValid,
  // `<input type="checkbox" disabled="">`, optionally `checked=""` (TaskListMarker).
  input: (el) => {
    const k = shapeKey(el);
    return (
      (k === "disabled,type" || k === "checked,disabled,type") &&
      el.getAttribute("type") === "checkbox"
    );
  },
  // `<img data-cid alt>`, with `width` and `height` together or not at all (lfm-1.md §6.2).
  img: (el) => {
    const k = shapeKey(el);
    return k === "alt,data-cid" || k === "alt,data-cid,height,width";
  },
  // `span.media[data-cid]`, `span.media-missing`, `span.link-host`, or `span[data-tex]`.
  span: (el) => {
    const k = shapeKey(el);
    const cls = el.getAttribute("class");
    if (k === "class") return cls === "media-missing" || cls === "link-host";
    if (k === "class,data-cid") return cls === "media";
    return k === "data-tex";
  },
};

/** Void elements are removed when malformed; others lose every attribute, which leaves them
 * inert (an `a` without `href` is not a link). */
const REMOVE_WHEN_MALFORMED = new Set(["input", "img"]);

/** The `afterSanitizeAttributes` hook: whole-element shapes (lfm-1.md §8.1). */
function enforceShape(node: Node): void {
  if (node.nodeType !== 1) return; // Node.ELEMENT_NODE
  const el = node as Element;
  if (el.namespaceURI !== HTML_NS) return;
  const check = SHAPES[el.localName];
  if (!check || check(el)) return;
  if (REMOVE_WHEN_MALFORMED.has(el.localName)) {
    el.remove();
    return;
  }
  for (const name of el.getAttributeNames()) el.removeAttribute(name);
}

let purifier: Purifier | null = null;

/** The dedicated, configured DOMPurify instance (created on first use). */
function lfmPurifier(): Purifier {
  if (purifier) return purifier;
  const p = DOMPurify(window);
  p.setConfig(LFM_CONFIG);
  p.addHook("uponSanitizeAttribute", (node, data) => {
    if (!attributeAllowed(node as Element, data.attrName, data.attrValue)) data.keepAttr = false;
  });
  p.addHook("afterSanitizeAttributes", enforceShape);
  purifier = p;
  return p;
}

/** `DOMPurify.sanitize(html, LFM_CONFIG)` as a string. */
export function sanitizeLfm(html: string): string {
  return lfmPurifier().sanitize(html) as unknown as string;
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
