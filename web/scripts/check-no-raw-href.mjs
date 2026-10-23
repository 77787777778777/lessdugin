#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Static checks for the SPA's HTML and URL sinks (threat-model.md SPA-2, SPA-4; CSP style-src),
// over the parsed TS/TSX of every file in web/src and packages/engine/src (Vite's oxc parser, so
// strings, comments and JSX text never cause false hits):
//   raw-href      a URL-bearing attribute (`href`, `xlinkHref`/`xlink:href`, `src`, `srcSet`,
//                 `srcDoc`, `action`, `formAction`, `poster`, `ping`) on an intrinsic JSX element
//                 (<a>, <img>, <form>, …) outside src/components/SafeLink.tsx;
//   jsx-spread    `{...props}` on an intrinsic JSX element (it could carry any of the above);
//   dom-url       a DOM write that sets a URL or navigates: `x.href = …` and the other URL
//                 properties, `setAttribute("href" | "src" | "style" | "srcdoc" | "on…", …)` (or
//                 a non-literal attribute name), `setAttributeNS`, assignments to `location` /
//                 `location.href`, `location.assign` / `replace`, `window.open`, and
//                 `createElement("a", { href })`; the allowed places are the object-URL
//                 download helper in src/lib/download.ts and, for `location.assign` only, the
//                 sign-in redirect in src/lib/navigate.ts;
//   dsih          `dangerouslySetInnerHTML` anywhere (attribute, property or identifier);
//   html-sink     `innerHTML` / `outerHTML` / `insertAdjacentHTML` / `createContextualFragment` /
//                 `setHTMLUnsafe` / `document.write` outside src/lib/sanitize.ts (the Trusted
//                 Types sink behind <RenderedHtml>), tests aside;
//   eval          `eval`, `Function(…)`, string-argument `setTimeout` / `setInterval`;
//   inline-style  a JSX `style` attribute anywhere (CSP style-src 'self').
// Router links (`<Link to=…>`) never take a raw URL, so they are fine; so are components such as
// <SafeLink href={…}>, which take an engine `Href`.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAst } from "vite";

const SAFE_LINK = "components/SafeLink.tsx";
const SANITIZE = "lib/sanitize.ts";
const DOWNLOAD = "lib/download.ts";
const NAVIGATE = "lib/navigate.ts";
const SINKS = new Set([
  "innerHTML",
  "outerHTML",
  "insertAdjacentHTML",
  "createContextualFragment",
  "setHTMLUnsafe",
]);
/** URL-bearing JSX attributes (React names and their HTML spellings). */
const JSX_URL_ATTRS = new Set([
  "href",
  "xlinkHref",
  "xlink:href",
  "src",
  "srcSet",
  "srcset",
  "srcDoc",
  "srcdoc",
  "action",
  "formAction",
  "formaction",
  "poster",
  "ping",
]);
/** DOM properties whose assignment loads or navigates to a URL. */
const DOM_URL_PROPS = new Set([
  "href",
  "src",
  "srcset",
  "srcdoc",
  "action",
  "formAction",
  "poster",
  "ping",
]);
/** Attribute names `setAttribute` must not write. */
const DOM_URL_ATTRS = new Set([
  "href",
  "xlink:href",
  "src",
  "srcset",
  "srcdoc",
  "action",
  "formaction",
  "poster",
  "ping",
  "data",
  "style",
]);

function jsxAttrName(node) {
  const n = node.name;
  if (!n) return null;
  if (n.type === "JSXIdentifier") return n.name;
  if (n.type === "JSXNamespacedName") return `${n.namespace.name}:${n.name.name}`;
  return null;
}

function propertyName(node) {
  if (node.computed) return node.property?.type === "Literal" ? String(node.property.value) : null;
  return node.property?.name ?? null;
}

function stringValue(node) {
  if (node?.type === "Literal" && typeof node.value === "string") return node.value;
  if (node?.type === "TemplateLiteral" && node.expressions.length === 0)
    return node.quasis.map((q) => q.value.cooked).join("");
  return null;
}

const isIdent = (node, name) => node?.type === "Identifier" && node.name === name;

/** `location`, `window.location`, `document.location`, `globalThis.location`, `self.location`. */
function isLocation(node) {
  if (isIdent(node, "location")) return true;
  return (
    node?.type === "MemberExpression" &&
    propertyName(node) === "location" &&
    ["window", "document", "globalThis", "self", "top", "parent"].some((n) =>
      isIdent(node.object, n),
    )
  );
}

function* nodes(value) {
  const stack = [value];
  while (stack.length > 0) {
    const v = stack.pop();
    if (Array.isArray(v)) {
      for (let i = v.length - 1; i >= 0; i--) stack.push(v[i]);
    } else if (v && typeof v === "object") {
      if (typeof v.type === "string") yield v;
      for (const key of Object.keys(v)) {
        const child = v[key];
        if (child && typeof child === "object") stack.push(child);
      }
    }
  }
}

/** Problems in one file; `file` is relative to its source root with forward slashes. */
export function check(file, text) {
  const isTest = /\.test\.tsx?$/.test(file);
  const lang = file.endsWith(".tsx") ? "tsx" : "ts";
  const ast = parseAst(text, { lang });
  const problems = [];
  const lineOf = (offset) => text.slice(0, offset).split("\n").length;
  const report = (node, rule, message) =>
    problems.push({ file, line: lineOf(node.start), rule, message });
  const domUrl = (node, what) => {
    if (file !== DOWNLOAD && !isTest) {
      report(
        node,
        "dom-url",
        `${what}: URLs reach the DOM only through <SafeLink> (or lib/download.ts for object URLs)`,
      );
    }
  };
  for (const node of nodes(ast)) {
    if (node.type === "JSXOpeningElement") {
      // Intrinsic elements (<a>, <link>, <area>, …) are lowercase; components such as
      // <SafeLink href={…}> take an engine `Href`, never a raw URL string.
      const intrinsic = node.name?.type === "JSXIdentifier" && /^[a-z]/.test(node.name.name);
      for (const attr of node.attributes ?? []) {
        if (attr.type === "JSXSpreadAttribute") {
          if (intrinsic) {
            report(
              attr,
              "jsx-spread",
              "spread props on an intrinsic element: pass attributes explicitly",
            );
          }
          continue;
        }
        if (attr.type !== "JSXAttribute") continue;
        const name = jsxAttrName(attr);
        if (name !== null && JSX_URL_ATTRS.has(name) && intrinsic && file !== SAFE_LINK) {
          report(
            attr,
            "raw-href",
            `raw ${name} attribute: render links through <SafeLink> (or a router <Link to>)`,
          );
        } else if (name === "style") {
          report(
            attr,
            "inline-style",
            "inline style attribute (CSP style-src 'self'): use a CSS module class",
          );
        } else if (name === "dangerouslySetInnerHTML") {
          report(
            attr,
            "dsih",
            "dangerouslySetInnerHTML is banned: use <RenderedHtml> (Trusted Types + DOMPurify)",
          );
        }
      }
    } else if (
      (node.type === "Identifier" || node.type === "JSXIdentifier") &&
      node.name === "dangerouslySetInnerHTML"
    ) {
      report(
        node,
        "dsih",
        "dangerouslySetInnerHTML is banned: use <RenderedHtml> (Trusted Types + DOMPurify)",
      );
    } else if (node.type === "MemberExpression" && file !== SANITIZE && !isTest) {
      const prop = propertyName(node);
      const isWrite = (prop === "write" || prop === "writeln") && isIdent(node.object, "document");
      if ((prop && SINKS.has(prop)) || isWrite) {
        report(
          node,
          "html-sink",
          "HTML sink outside src/lib/sanitize.ts: render HTML through <RenderedHtml>",
        );
      }
    } else if (node.type === "AssignmentExpression") {
      const left = node.left;
      if (isLocation(left)) {
        domUrl(node, "assignment to location");
      } else if (left?.type === "MemberExpression") {
        const prop = propertyName(left);
        if (prop !== null && DOM_URL_PROPS.has(prop)) {
          if (prop === "href" && isLocation(left.object))
            domUrl(node, "assignment to location.href");
          else domUrl(node, `assignment to .${prop}`);
        } else if (prop === "hash" && isLocation(left.object)) {
          domUrl(node, "assignment to location.hash");
        }
      }
    } else if (node.type === "CallExpression" || node.type === "NewExpression") {
      const callee = node.callee;
      const args = node.arguments ?? [];
      const method = callee?.type === "MemberExpression" ? propertyName(callee) : null;
      if (
        node.type === "CallExpression" &&
        (method === "setAttribute" || method === "setAttributeNS")
      ) {
        const nameArg = method === "setAttribute" ? args[0] : args[1];
        const name = stringValue(nameArg)?.toLowerCase();
        if (name === undefined) domUrl(node, `${method} with a computed attribute name`);
        else if (DOM_URL_ATTRS.has(name) || name.startsWith("on")) {
          domUrl(node, `${method}("${name}")`);
        }
      } else if (
        node.type === "CallExpression" &&
        (method === "assign" || method === "replace") &&
        isLocation(callee.object)
      ) {
        if (!(file === NAVIGATE && method === "assign")) domUrl(node, `location.${method}`);
      } else if (
        node.type === "CallExpression" &&
        ((method === "open" &&
          ["window", "globalThis", "self"].some((n) => isIdent(callee.object, n))) ||
          isIdent(callee, "open"))
      ) {
        domUrl(node, "window.open");
      } else if (node.type === "CallExpression" && method === "createElement") {
        const props = args[1];
        if (props?.type === "ObjectExpression") {
          for (const p of props.properties ?? []) {
            const key = p.type === "Property" ? (p.key?.name ?? stringValue(p.key)) : "(spread)";
            if (key === "(spread)" || JSX_URL_ATTRS.has(key) || key === "style") {
              domUrl(p, `createElement(…, { ${key} })`);
            }
          }
        }
      }
      if (isIdent(callee, "eval") || isIdent(callee, "Function")) {
        report(node, "eval", "code from strings (eval / Function) is banned");
      } else if (
        node.type === "CallExpression" &&
        (isIdent(callee, "setTimeout") ||
          isIdent(callee, "setInterval") ||
          method === "setTimeout" ||
          method === "setInterval") &&
        stringValue(args[0]) !== null
      ) {
        report(node, "eval", "string-argument timers evaluate code: pass a function");
      }
    }
  }
  // Duplicates arise when an attribute name node is also visited as an identifier.
  const seen = new Set();
  return problems.filter((p) => {
    const k = `${p.line}:${p.rule}`;
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else if (/\.(ts|tsx)$/.test(name) && !name.endsWith(".d.ts")) yield p;
  }
}

/** The source roots checked, with the prefix used in messages. */
export const ROOTS = [
  { dir: fileURLToPath(new URL("../src/", import.meta.url)), label: "web/src" },
  {
    dir: fileURLToPath(new URL("../../packages/engine/src/", import.meta.url)),
    label: "packages/engine/src",
  },
];

if (import.meta.url === `file://${process.argv[1]}`) {
  const problems = [];
  for (const { dir, label } of ROOTS) {
    for (const path of walk(dir)) {
      const rel = relative(dir, path).split(sep).join("/");
      for (const p of check(rel, readFileSync(path, "utf8"))) problems.push({ ...p, label });
    }
  }
  for (const p of problems)
    console.error(`${p.label}/${p.file}:${p.line}: [${p.rule}] ${p.message}`);
  if (problems.length > 0) {
    console.error(`check-no-raw-href: ${problems.length} problem(s)`);
    process.exit(1);
  }
  console.log("check-no-raw-href: ok");
}
