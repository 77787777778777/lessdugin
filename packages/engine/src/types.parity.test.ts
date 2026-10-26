// SPDX-License-Identifier: AGPL-3.0-or-later
// Drift guard: the hand-written DTO types of ./types.ts mirror the Rust ones exactly.
//
// testdata/dto/dto-1.d.ts is generated from the Rust types by their tsify derives
// (crates/wasm/tests/dto_decl.rs, which fails while the file is stale). This test parses that
// file and ./types.ts and compares every generated type with the hand-written one of the same
// name: the fields of each object, their optionality (`?:`: the serialiser may omit the field or
// the deserialiser may go without it), their types, the variants of each union (tagged enums by
// `kind`) and the values of each string enum. Aliases of `types.ts` (`AtUri`, `Did`, `Ms`, …)
// are transparent; a named DTO is compared once, under its own name.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const GENERATED = fileURLToPath(new URL("../../../testdata/dto/dto-1.d.ts", import.meta.url));
const TYPES_TS = fileURLToPath(new URL("./types.ts", import.meta.url));

/** Generated names that types.ts spells differently. */
const RENAMED: Readonly<Record<string, string>> = {
  Event: "EngineEvent",
  ErrorCode: "EngineErrorCode",
  Value: "Json",
};

/** Fields whose types.ts type is deliberately narrower than the Rust type (presence and
 * optionality are still compared). */
const REFINED: Readonly<Record<string, string>> = {
  "EngineError.detail": "always a JSON object (`serde_json::Value` in Rust)",
};

/** Types of types.ts with no Rust counterpart (yet), and why. Every other exported type must
 * be a generated one. */
const TS_ONLY: Readonly<Record<string, string>> = {
  Did: "alias of string",
  AtUri: "alias of string",
  Cid: "alias of string",
  Slug: "alias of string",
  Ms: "alias of number",
  Href: "lessdugin-markdown's safe_href result, re-tagged by the wasm binding",
  HrefContext: "an argument of safeHref",
  SearchQuery: "search (M5)",
  SequenceChapterDraft: "sequences (M5)",
  SequenceDraft: "sequences (M5)",
  SequencePosition: "sequences (M5)",
  WikiDraft: "the wiki (M5)",
  TagVoteValue: "alias of KarmaValue",
  RkeyInput: "deriveRkey (M4)",
  DraftEntry: "drafts (M4)",
  SettingKey: "settings (M4)",
  RenderOpts: "an argument of render",
  ValidationReport: "validateRecord (M4)",
  EventListener: "the client's event callback",
  Unsubscribe: "the client's event callback",
};

type Ty =
  | { k: "prim"; name: string }
  | { k: "lit"; value: string }
  | { k: "ref"; name: string; args: Ty[] }
  | { k: "array"; elem: Ty }
  | { k: "tuple"; elems: Ty[] }
  | { k: "object"; props: Map<string, { opt: boolean; type: Ty }>; index: Ty | null }
  | { k: "union"; members: Ty[] }
  | { k: "inter"; members: Ty[] };

const PRIMS = new Set([
  "string",
  "number",
  "boolean",
  "null",
  "undefined",
  "unknown",
  "any",
  "never",
  "true",
  "false",
]);

const TOKEN = /"(?:[^"\\]|\\.)*"|=>|[A-Za-z_$][\w$]*|\d+(?:\.\d+)?|[^\s]/y;

/** A recursive-descent parser for the type declarations both files use. */
class Parser {
  private peeked: string | null = null;

  constructor(
    private readonly src: string,
    private pos: number,
  ) {}

  private read(): string {
    for (;;) {
      while (this.pos < this.src.length && /\s/.test(this.src[this.pos] ?? "")) this.pos++;
      if (this.src.startsWith("//", this.pos)) {
        const end = this.src.indexOf("\n", this.pos);
        this.pos = end < 0 ? this.src.length : end;
      } else if (this.src.startsWith("/*", this.pos)) {
        const end = this.src.indexOf("*/", this.pos + 2);
        this.pos = end < 0 ? this.src.length : end + 2;
      } else break;
    }
    if (this.pos >= this.src.length) return "";
    TOKEN.lastIndex = this.pos;
    const m = TOKEN.exec(this.src);
    if (!m) throw new Error(`no token at ${this.pos}`);
    this.pos = TOKEN.lastIndex;
    return m[0];
  }

  peek(): string {
    this.peeked ??= this.read();
    return this.peeked;
  }

  next(): string {
    const t = this.peek();
    this.peeked = null;
    return t;
  }

  expect(t: string): void {
    const got = this.next();
    if (got !== t) throw new Error(`expected ${t}, got ${JSON.stringify(got)} near ${this.pos}`);
  }

  /** `export interface Name { … }` or `export type Name = …;` */
  declaration(): Ty {
    this.expect("export");
    const kw = this.next();
    this.next(); // the name
    if (kw === "interface") {
      this.expect("{");
      return this.objectBody();
    }
    if (kw !== "type") throw new Error(`not a declaration: ${kw}`);
    this.expect("=");
    return this.type();
  }

  type(): Ty {
    if (this.peek() === "|") this.next();
    const members = [this.intersection()];
    while (this.peek() === "|") {
      this.next();
      members.push(this.intersection());
    }
    return members.length === 1 ? (members[0] as Ty) : { k: "union", members };
  }

  private intersection(): Ty {
    const members = [this.postfix()];
    while (this.peek() === "&") {
      this.next();
      members.push(this.postfix());
    }
    return members.length === 1 ? (members[0] as Ty) : { k: "inter", members };
  }

  private postfix(): Ty {
    let t = this.primary();
    while (this.peek() === "[") {
      this.next();
      this.expect("]");
      t = { k: "array", elem: t };
    }
    return t;
  }

  private primary(): Ty {
    const t = this.next();
    if (t === "{") return this.objectBody();
    if (t === "(") {
      const inner = this.type();
      this.expect(")");
      return inner;
    }
    if (t === "[") {
      const elems: Ty[] = [];
      while (this.peek() !== "]") {
        elems.push(this.type());
        if (this.peek() === ",") this.next();
      }
      this.expect("]");
      return { k: "tuple", elems };
    }
    if (t.startsWith('"')) return { k: "lit", value: JSON.parse(t) as string };
    if (/^\d/.test(t)) return { k: "lit", value: t };
    if (PRIMS.has(t)) return { k: "prim", name: t };
    if (/^[A-Za-z_$]/.test(t)) {
      const args: Ty[] = [];
      if (this.peek() === "<") {
        this.next();
        while (this.peek() !== ">") {
          args.push(this.type());
          if (this.peek() === ",") this.next();
        }
        this.expect(">");
      }
      return { k: "ref", name: t, args };
    }
    throw new Error(`unexpected ${JSON.stringify(t)} near ${this.pos}`);
  }

  private objectBody(): Ty {
    const props = new Map<string, { opt: boolean; type: Ty }>();
    let index: Ty | null = null;
    while (this.peek() !== "}") {
      if (this.peek() === "readonly") this.next();
      if (this.peek() === "[") {
        this.next();
        this.next(); // the key's name
        this.expect(":");
        this.type();
        this.expect("]");
        this.expect(":");
        index = this.type();
      } else {
        const raw = this.next();
        const key = raw.startsWith('"') ? (JSON.parse(raw) as string) : raw;
        const opt = this.peek() === "?";
        if (opt) this.next();
        this.expect(":");
        props.set(key, { opt, type: this.type() });
      }
      if (this.peek() === ";" || this.peek() === ",") this.next();
    }
    this.expect("}");
    return { k: "object", props, index };
  }
}

/** The exported type declarations of a file, parsed on demand. */
class Declarations {
  private readonly at = new Map<string, number>();
  private readonly parsed = new Map<string, Ty>();

  constructor(private readonly src: string) {
    for (const m of src.matchAll(/^export (?:interface|type) (\w+)/gm)) {
      if (m[1]) this.at.set(m[1], m.index);
    }
  }

  names(): string[] {
    return [...this.at.keys()];
  }

  has(name: string): boolean {
    return this.at.has(name);
  }

  get(name: string): Ty | undefined {
    const cached = this.parsed.get(name);
    if (cached) return cached;
    const at = this.at.get(name);
    if (at === undefined) return undefined;
    const t = new Parser(this.src, at).declaration();
    this.parsed.set(name, t);
    return t;
  }
}

function show(t: Ty): string {
  switch (t.k) {
    case "prim":
      return t.name;
    case "lit":
      return JSON.stringify(t.value);
    case "ref":
      return t.name;
    case "array":
      return `${show(t.elem)}[]`;
    case "tuple":
      return `[${t.elems.map(show).join(", ")}]`;
    case "object":
      return `{ ${[...t.props].map(([k, p]) => `${k}${p.opt ? "?" : ""}`).join(", ")} }`;
    case "union":
      return t.members.map(show).join(" | ");
    case "inter":
      return t.members.map(show).join(" & ");
  }
}

/** What identifies a union member: its `kind`, its literal, its name or its field names. */
function memberKey(t: Ty, rename: (n: string) => string): string {
  switch (t.k) {
    case "object": {
      const kind = t.props.get("kind");
      if (kind?.type.k === "lit") return `kind ${kind.type.value}`;
      return `{ ${[...t.props.keys()].sort().join(", ")} }`;
    }
    case "lit":
      return JSON.stringify(t.value);
    case "ref":
      return rename(t.name);
    case "array":
      return `${memberKey(t.elem, rename)}[]`;
    default:
      return show(t);
  }
}

class Comparison {
  readonly drift: string[] = [];

  constructor(
    private readonly gen: Declarations,
    private readonly ts: Declarations,
  ) {}

  private tsName(n: string): string {
    return RENAMED[n] ?? n;
  }

  private resolve(t: Ty, side: Declarations, path: string): Ty | null {
    if (t.k !== "ref") return t;
    const d = side.get(t.name);
    if (!d) {
      this.drift.push(
        `${path}: ${t.name} is not declared in ${side === this.gen ? "dto-1.d.ts" : "types.ts"}`,
      );
      return null;
    }
    return d;
  }

  compare(g: Ty, t: Ty, path: string, depth = 0): void {
    if (depth > 40) {
      this.drift.push(`${path}: nested too deeply`);
      return;
    }
    // a named DTO on both sides: compared once, under its own name
    if (g.k === "ref" && t.k === "ref" && this.tsName(g.name) === t.name) return;
    const gr = this.resolve(g, this.gen, path);
    const tr = this.resolve(t, this.ts, path);
    if (!gr || !tr) return;
    if (gr !== g || tr !== t) {
      this.compare(gr, tr, path, depth + 1);
      return;
    }
    // a one-variant side of a union comparison
    if (g.k === "union" && t.k !== "union") {
      this.unions(g, { k: "union", members: [t] }, path, depth);
      return;
    }
    if (t.k === "union" && g.k !== "union") {
      this.unions({ k: "union", members: [g] }, t, path, depth);
      return;
    }
    if (g.k !== t.k) {
      this.drift.push(`${path}: Rust ${show(g)}, types.ts ${show(t)}`);
      return;
    }
    switch (g.k) {
      case "prim":
      case "lit":
        if (show(g) !== show(t)) this.drift.push(`${path}: Rust ${show(g)}, types.ts ${show(t)}`);
        return;
      case "array":
        this.compare(g.elem, (t as typeof g).elem, `${path}[]`, depth + 1);
        return;
      case "tuple": {
        const te = (t as typeof g).elems;
        if (te.length !== g.elems.length) {
          this.drift.push(`${path}: Rust ${show(g)}, types.ts ${show(t)}`);
          return;
        }
        g.elems.forEach((e, i) => {
          this.compare(e, te[i] as Ty, `${path}[${i}]`, depth + 1);
        });
        return;
      }
      case "object":
        this.objects(g, t as typeof g, path, depth);
        return;
      case "union":
        this.unions(g, t as typeof g, path, depth);
        return;
      default:
        this.drift.push(`${path}: cannot compare ${show(g)}`);
    }
  }

  private objects(g: Extract<Ty, { k: "object" }>, t: typeof g, path: string, depth: number): void {
    for (const [key, gp] of g.props) {
      const tp = t.props.get(key);
      const at = `${path}.${key}`;
      if (!tp) {
        this.drift.push(`${at}: missing in types.ts`);
        continue;
      }
      if (gp.opt !== tp.opt) {
        this.drift.push(
          `${at}: ${gp.opt ? "optional" : "required"} in Rust, ${tp.opt ? "optional" : "required"} in types.ts`,
        );
      }
      if (!(at in REFINED)) this.compare(gp.type, tp.type, at, depth + 1);
    }
    for (const key of t.props.keys()) {
      if (!g.props.has(key)) this.drift.push(`${path}.${key}: not in the Rust type`);
    }
    if ((g.index === null) !== (t.index === null)) {
      this.drift.push(`${path}: index signature in only one of them`);
    } else if (g.index && t.index) {
      this.compare(g.index, t.index, `${path}[key]`, depth + 1);
    }
  }

  private unions(g: Extract<Ty, { k: "union" }>, t: typeof g, path: string, depth: number): void {
    const keyed = (members: Ty[], rename: (n: string) => string) =>
      new Map(members.map((m) => [memberKey(m, rename), m] as const));
    const gm = keyed(g.members, (n) => this.tsName(n));
    const tm = keyed(t.members, (n) => n);
    for (const [key, m] of gm) {
      const other = tm.get(key);
      if (!other) this.drift.push(`${path}: variant ${key} missing in types.ts`);
      else this.compare(m, other, `${path}<${key}>`, depth + 1);
    }
    for (const key of tm.keys()) {
      if (!gm.has(key)) this.drift.push(`${path}: variant ${key} not in the Rust type`);
    }
  }
}

describe("types.ts mirrors the Rust DTOs (testdata/dto/dto-1.d.ts)", () => {
  const gen = new Declarations(readFileSync(GENERATED, "utf8"));
  const ts = new Declarations(readFileSync(TYPES_TS, "utf8"));

  it("parses every declaration of both files", () => {
    expect(gen.names().length).toBeGreaterThan(120);
    for (const n of gen.names()) expect(gen.get(n), n).toBeDefined();
    for (const n of ts.names()) {
      if (n in TS_ONLY && ["EventListener", "Unsubscribe"].includes(n)) continue;
      expect(ts.get(n), n).toBeDefined();
    }
  });

  it("declares every Rust type, with the same fields, optionality, types and variants", () => {
    const cmp = new Comparison(gen, ts);
    for (const name of gen.names()) {
      const tsName = RENAMED[name] ?? name;
      const t = ts.get(tsName);
      const g = gen.get(name);
      if (!t) cmp.drift.push(`${tsName}: missing in types.ts`);
      else if (g) cmp.compare(g, t, tsName);
    }
    expect(cmp.drift).toEqual([]);
  });

  it("has no other types than the Rust ones, apart from the listed TS-only ones", () => {
    const fromRust = new Set(gen.names().map((n) => RENAMED[n] ?? n));
    const extra = ts.names().filter((n) => !fromRust.has(n) && !(n in TS_ONLY));
    expect(extra).toEqual([]);
    const stale = Object.keys(TS_ONLY).filter((n) => !ts.has(n) || fromRust.has(n));
    expect(stale).toEqual([]);
  });

  it("catches drift", () => {
    const g = new Declarations(
      'export interface A { x: string; y?: B; z: C }\nexport type B = "p" | "q";\n' +
        'export type C = { kind: "one"; n: number } | { kind: "two" };\n',
    );
    const t = new Declarations(
      'export interface A { x?: string; y?: B; z: C; w: number }\nexport type B = "p";\n' +
        'export type C = { kind: "one"; n: string } | { kind: "three" };\n',
    );
    const cmp = new Comparison(g, t);
    for (const n of g.names()) cmp.compare(g.get(n) as Ty, t.get(n) as Ty, n);
    expect(cmp.drift).toEqual([
      "A.x: required in Rust, optional in types.ts",
      "A.w: not in the Rust type",
      'B: variant "q" missing in types.ts',
      "C<kind one>.n: Rust number, types.ts string",
      "C: variant kind two missing in types.ts",
      "C: variant kind three not in the Rust type",
    ]);
  });
});
