// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The native `outbox:<did>` lock of `lessdugin web` mode, held for this tab through
// `POST /bridge/lock` (threat-model §7.4; the wire format is crates/cli/src/web/lock.rs's). In
// bridge mode the tab's outbox rows live in the native `state.sqlite`, so a Web Lock alone does
// not keep a native client (`lessdugin sync`, `outbox flush`, the TUI) from pumping the same
// rows (conformance review M1, sync.md §7.1 "one write in flight per account"):
//
//   POST /bridge/lock {"name": "outbox:<did>", "wait": bool}
//     held      → 200, a body that does not end: {"held":true}\n, then \n every beat
//     not held  → 200 {"held":false} (busy, or the server's wait ran out: ask again)
//
// The lock is held exactly as long as the response body is open; releasing aborts the request.
// The host (host.ts) takes the tab's Web Lock `lessdugin-outbox-<did>` first and then this one,
// and releases both when the Rust guard drops.

import type { LockHandle } from "./locks.ts";
import { hostError, isHostError } from "./protocol.ts";

export interface BridgeLockOptions {
  /** `POST` endpoint, e.g. `/bridge/lock` (same origin). */
  endpoint: string;
  /** The bridge session's bearer token. */
  token: string;
  fetch?: typeof fetch;
  /** A held lock's body ended before it was released (the server stopped, the session was
   * revoked, the connection broke). */
  onLost?: (name: string) => void;
  /** Pause before asking again after `{"held":false}` while waiting (default 100 ms). */
  retryMs?: number;
}

/** Host lock names that are also native locks (the server bridges no other). */
export function isBridgedLock(name: string): boolean {
  return name.startsWith("outbox:");
}

/** The longest first line read from the server. */
const MAX_LINE = 1024;

type Reader = ReadableStreamDefaultReader<Uint8Array>;

/** The first line of `reader` (or the whole body when it has no newline), at most `MAX_LINE`
 * bytes; the reader stays open after a newline. */
async function firstLine(reader: Reader): Promise<{ line: string; ended: boolean }> {
  const parts: number[] = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return { line: new TextDecoder().decode(new Uint8Array(parts)), ended: true };
    for (const b of value) {
      if (b === 0x0a)
        return { line: new TextDecoder().decode(new Uint8Array(parts)), ended: false };
      parts.push(b);
      if (parts.length > MAX_LINE) throw hostError("storage", "bridge lock: answer too long");
    }
  }
}

/** Takes the native lock `name` for this tab; `null` when it is busy and `!wait`. While
 * `wait`, asks again until the lock is taken. */
export async function takeBridgeLock(
  opts: BridgeLockOptions,
  name: string,
  wait: boolean,
): Promise<LockHandle | null> {
  const fetchFn = opts.fetch ?? globalThis.fetch.bind(globalThis);
  for (;;) {
    const abort = new AbortController();
    let resp: Response;
    try {
      resp = await fetchFn(opts.endpoint, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: `Bearer ${opts.token}` },
        body: JSON.stringify({ name, wait }),
        cache: "no-store",
        credentials: "omit",
        redirect: "error",
        signal: abort.signal,
      });
    } catch (e) {
      throw hostError("storage", `bridge lock: ${e instanceof Error ? e.message : String(e)}`);
    }
    if (resp.status === 401) {
      abort.abort();
      throw hostError("authRequired", "bridge session expired");
    }
    if (!resp.ok || !resp.body) {
      const text = await resp.text().catch(() => "");
      throw hostError("storage", `bridge lock: HTTP ${resp.status} ${text.slice(0, 200)}`);
    }
    const reader = resp.body.getReader();
    let answer: { line: string; ended: boolean };
    try {
      answer = await firstLine(reader);
    } catch (e) {
      abort.abort();
      throw isHostError(e)
        ? e
        : hostError("storage", `bridge lock: ${e instanceof Error ? e.message : String(e)}`);
    }
    let held: unknown;
    try {
      held = (JSON.parse(answer.line) as { held?: unknown }).held;
    } catch {
      held = undefined;
    }
    if (held === true && !answer.ended) return holding(opts, name, reader, abort);
    abort.abort();
    if (held !== false && held !== true) throw hostError("storage", "bridge lock: bad answer");
    // `{"held":true}` with an ended body was released at once: treat it as busy
    if (!wait) return null;
    await new Promise((r) => setTimeout(r, opts.retryMs ?? 100));
  }
}

/** A held lock: the heartbeats are read (and dropped) until it is released. */
function holding(
  opts: BridgeLockOptions,
  name: string,
  reader: Reader,
  abort: AbortController,
): LockHandle {
  let released = false;
  void (async () => {
    try {
      for (;;) {
        const { done } = await reader.read();
        if (done) break;
      }
    } catch {
      // aborted, or the connection broke
    }
    if (!released) opts.onLost?.(name);
  })();
  return {
    release() {
      if (released) return;
      released = true;
      abort.abort();
      reader.cancel().catch(() => {});
    },
  };
}
