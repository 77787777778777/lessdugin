// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The `state` store of `lessdugin web` mode: the native `state.sqlite` behind
// `POST /bridge/kv` (threat-model §7.4; the wire format is crates/cli/src/web/kv.rs's), so
// drafts, the outbox and read marks never live in the browser storage of the bridge's origin.
// Keys and values travel as standard base64 with padding:
//
//   {"op": "get_many", "keys": [k…]}                                 → {"values": [v | null…]}
//   {"op": "scan", "start": B, "end": B, "dir": "forward"|"reverse", "limit": n}
//                                                                     → {"entries": [[k, v]…]}
//   {"op": "write", "batch": [{"put": [k, v]} | {"delete": k} | {"deleteRange": [B, B]}…]} → {}
//   B = {"included": k} | {"excluded": k}
//
// The bridge serves only bounded ranges inside its key scopes; an unbounded side is sent as
// the range's prefix successor bound computed by the core (`KeyRange::prefix` always yields
// bounded ranges for the state keys).

import { decodeOps, type KvBackend, rangeIsEmpty } from "./idb.ts";
import { hostError } from "./protocol.ts";

export function toBase64(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 0x8000) {
    s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  }
  return btoa(s);
}

export function fromBase64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

type Bound = { included: string } | { excluded: string };

function bound(k: Uint8Array | undefined, open: boolean, side: string): Bound {
  if (k === undefined) {
    throw hostError("unsupported", `the bridge store takes bounded ranges (no ${side} bound)`);
  }
  return open ? { excluded: toBase64(k) } : { included: toBase64(k) };
}

export interface BridgeKvOptions {
  /** `POST` endpoint, e.g. `/bridge/kv` (same origin). */
  endpoint: string;
  /** The bridge session's bearer token. */
  token: string;
  fetch?: typeof fetch;
}

/** A `KvBackend` over `POST /bridge/kv`. */
export class BridgeKv implements KvBackend {
  private readonly fetch: typeof fetch;

  constructor(private readonly opts: BridgeKvOptions) {
    this.fetch = opts.fetch ?? globalThis.fetch.bind(globalThis);
  }

  private async post(body: unknown): Promise<unknown> {
    let resp: Response;
    try {
      resp = await this.fetch(this.opts.endpoint, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization: `Bearer ${this.opts.token}`,
        },
        body: JSON.stringify(body),
        cache: "no-store",
        credentials: "omit",
      });
    } catch (e) {
      throw hostError("storage", `bridge store: ${e instanceof Error ? e.message : String(e)}`);
    }
    const text = await resp.text();
    if (resp.status === 401) throw hostError("authRequired", "bridge session expired");
    if (!resp.ok) {
      throw hostError("storage", `bridge store: HTTP ${resp.status} ${text.slice(0, 200)}`);
    }
    try {
      return JSON.parse(text) as unknown;
    } catch {
      throw hostError("storage", "bridge store: not JSON");
    }
  }

  async getMany(keys: Uint8Array[]): Promise<(Uint8Array | undefined)[]> {
    if (keys.length === 0) return [];
    const r = (await this.post({ op: "get_many", keys: keys.map(toBase64) })) as {
      values?: (string | null)[];
    };
    if (!Array.isArray(r.values) || r.values.length !== keys.length) {
      throw hostError("storage", "bridge store: bad get_many answer");
    }
    return r.values.map((v) => (v === null ? undefined : fromBase64(v)));
  }

  async scan(
    lower: Uint8Array | undefined,
    lowerOpen: boolean,
    upper: Uint8Array | undefined,
    upperOpen: boolean,
    reverse: boolean,
    limit: number,
  ): Promise<Uint8Array[]> {
    if (limit <= 0 || rangeIsEmpty({ lower, lowerOpen, upper, upperOpen })) return [];
    const r = (await this.post({
      op: "scan",
      start: bound(lower, lowerOpen, "lower"),
      end: bound(upper, upperOpen, "upper"),
      dir: reverse ? "reverse" : "forward",
      limit,
    })) as { entries?: [string, string][] };
    if (!Array.isArray(r.entries)) throw hostError("storage", "bridge store: bad scan answer");
    const out: Uint8Array[] = [];
    for (const [k, v] of r.entries) out.push(fromBase64(k), fromBase64(v));
    return out;
  }

  async write(ops: unknown[]): Promise<void> {
    const batch = decodeOps(ops).flatMap((op): unknown[] => {
      if (op.op === "put") return [{ put: [toBase64(op.key), toBase64(op.value)] }];
      if (op.op === "delete") return [{ delete: toBase64(op.key) }];
      if (rangeIsEmpty(op.range)) return [];
      const r = op.range;
      return [
        {
          deleteRange: [bound(r.lower, r.lowerOpen, "lower"), bound(r.upper, r.upperOpen, "upper")],
        },
      ];
    });
    if (batch.length === 0) return;
    await this.post({ op: "write", batch });
  }
}
