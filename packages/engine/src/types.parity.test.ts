// SPDX-License-Identifier: AGPL-3.0-or-later
// Drift guard: until tsify generates them, the hand-written DTO types in ./types.ts must keep
// the field names of the Rust DTOs (`crates/core/src/dto.rs`) and the values of its string enums.
// This reads the Rust source textually (struct bodies of the `dto!` macro, `rename` and
// `skip_serializing_if` attributes, snake_case → camelCase), so it is a coarse check; tagged
// enums are compared by hand. A field the Rust side may omit (`skip_serializing_if`) must be
// optional (`?:`) in TypeScript, and every other field required.

import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const DTO_RS = fileURLToPath(new URL("../../../crates/core/src/dto.rs", import.meta.url));
const TYPES_TS = fileURLToPath(new URL("./types.ts", import.meta.url));

const camel = (s: string): string => s.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

/** A field name, with `?` appended when it is optional. */
function rustStructs(rs: string): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const m of rs.matchAll(/pub struct (\w+) \{([\s\S]*?)\n {4}\}/g)) {
    const fields: string[] = [];
    let rename: string | null = null;
    let optional = false;
    for (const line of (m[2] ?? "").split("\n")) {
      if (/^\s*#\[/.test(line)) {
        const r = /rename = "(\w+)"/.exec(line);
        if (r?.[1]) rename = r[1];
        if (line.includes("skip_serializing_if")) optional = true;
        continue;
      }
      const f = /^\s*pub (\w+): .+,/.exec(line);
      if (f?.[1]) {
        fields.push(`${rename ?? camel(f[1])}${optional ? "?" : ""}`);
        rename = null;
        optional = false;
      }
    }
    if (m[1]) out.set(m[1], fields);
  }
  return out;
}

function rustStringEnums(rs: string): Map<string, string[]> {
  const out = new Map<string, string[]>();
  // the bodies of `string_enum!` (one line or several)
  for (const m of rs.matchAll(/string_enum! \{[\s\S]*?pub enum (\w+) \{([^}]*)\}/g)) {
    const values = (m[2] ?? "")
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v !== "")
      .map((v) => v.charAt(0).toLowerCase() + v.slice(1));
    if (m[1]) out.set(m[1], values);
  }
  return out;
}

function tsInterfaces(ts: string): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const m of ts.matchAll(/export interface (\w+) \{([\s\S]*?)\n\}/g)) {
    const fields = [...(m[2] ?? "").matchAll(/^\s+(\w+\??):/gm)].map((f) => f[1] ?? "");
    if (m[1]) out.set(m[1], fields);
  }
  return out;
}

describe.skipIf(!existsSync(DTO_RS))("DTO parity with crates/core/src/dto.rs", () => {
  const rs = existsSync(DTO_RS) ? readFileSync(DTO_RS, "utf8") : "";
  const ts = readFileSync(TYPES_TS, "utf8");

  it("has every Rust DTO struct with the same fields and the same optionality", () => {
    const interfaces = tsInterfaces(ts);
    const structs = rustStructs(rs);
    expect(structs.size).toBeGreaterThan(40);
    const drift: string[] = [];
    for (const [name, fields] of structs) {
      const mine = interfaces.get(name);
      if (!mine) drift.push(`${name}: missing`);
      else if ([...fields].sort().join() !== [...mine].sort().join()) {
        drift.push(`${name}: rust [${fields.join(", ")}] ts [${mine.join(", ")}]`);
      }
    }
    expect(drift).toEqual([]);
  });

  it("has every Rust string enum with the same values", () => {
    const drift: string[] = [];
    const enums = rustStringEnums(rs);
    expect(enums.has("NotificationReason")).toBe(true);
    for (const [name, values] of enums) {
      const t = new RegExp(`export type ${name} =([^;]*);`).exec(ts);
      const mine = t?.[1] ? [...t[1].matchAll(/"(\w+)"/g)].map((v) => v[1]) : null;
      if (!mine || [...values].sort().join() !== [...mine].sort().join()) drift.push(name);
    }
    expect(drift).toEqual([]);
  });
});
