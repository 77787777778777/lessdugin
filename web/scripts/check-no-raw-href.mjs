#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Static checks for the SPA's HTML and URL sinks (threat-model.md SPA-2, SPA-4; CSP style-src),
// over the parsed TSX of every file in web/src (Vite's oxc parser, so strings, comments and JSX
// text never cause false hits):
//   raw-href      an `href` attribute on an intrinsic JSX element (<a>, <link>, …) outside
//                 src/components/SafeLink.tsx;
//   dsih          `dangerouslySetInnerHTML` anywhere (attribute, property or identifier);
//   html-sink     `innerHTML` / `outerHTML` / `insertAdjacentHTML` / `document.write` outside
//                 src/lib/sanitize.ts (the Trusted Types sink behind <RenderedHtml>), tests aside;
//   inline-style  a JSX `style` attribute anywhere (CSP style-src 'self').
// Router links (`<Link to=…>`) never take a raw URL, so they are fine.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAst } from "vite";

const SAFE_LINK = "components/SafeLink.tsx";
const SANITIZE = "lib/sanitize.ts";
const SINKS = new Set(["innerHTML", "outerHTML", "insertAdjacentHTML"]);

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

/** Problems in one file; `file` is relative to web/src with forward slashes. */
export function check(file, text) {
  const isTest = /\.test\.tsx?$/.test(file);
  const lang = file.endsWith(".tsx") ? "tsx" : "ts";
  const ast = parseAst(text, { lang });
  const problems = [];
  const lineOf = (offset) => text.slice(0, offset).split("\n").length;
  const report = (node, rule, message) =>
    problems.push({ file, line: lineOf(node.start), rule, message });
  for (const node of nodes(ast)) {
    if (node.type === "JSXOpeningElement") {
      // Intrinsic elements (<a>, <link>, <area>, …) are lowercase; components such as
      // <SafeLink href={…}> take an engine `Href`, never a raw URL string.
      const intrinsic = node.name?.type === "JSXIdentifier" && /^[a-z]/.test(node.name.name);
      for (const attr of node.attributes ?? []) {
        if (attr.type !== "JSXAttribute") continue;
        const name = jsxAttrName(attr);
        if (name === "href" && intrinsic && file !== SAFE_LINK) {
          report(
            attr,
            "raw-href",
            "raw href attribute: render links through <SafeLink> (or a router <Link to>)",
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
      const isWrite =
        (prop === "write" || prop === "writeln") &&
        node.object?.type === "Identifier" &&
        node.object.name === "document";
      if ((prop && SINKS.has(prop)) || isWrite) {
        report(
          node,
          "html-sink",
          "HTML sink outside src/lib/sanitize.ts: render HTML through <RenderedHtml>",
        );
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

if (import.meta.url === `file://${process.argv[1]}`) {
  const root = fileURLToPath(new URL("../src/", import.meta.url));
  const problems = [];
  for (const path of walk(root)) {
    const rel = relative(root, path).split(sep).join("/");
    problems.push(...check(rel, readFileSync(path, "utf8")));
  }
  for (const p of problems) console.error(`web/src/${p.file}:${p.line}: [${p.rule}] ${p.message}`);
  if (problems.length > 0) {
    console.error(`check-no-raw-href: ${problems.length} problem(s)`);
    process.exit(1);
  }
  console.log("check-no-raw-href: ok");
}
