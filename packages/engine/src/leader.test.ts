// SPDX-License-Identifier: AGPL-3.0-or-later
// One leader per origin (sync.md §10.6): the election hands over in queue order, the change
// relay carries only other tabs' `changed`/`write` events, and host locks map onto Web Locks.
import { describe, expect, it } from "vitest";
import { ChangeRelay, type ChannelLike, LeaderElection } from "./leader.ts";
import { acquireLock, LocalLocks, webLockName } from "./locks.ts";
import { LEADER_LOCK } from "./protocol.ts";
import type { EngineEvent } from "./types.ts";

const tick = () => new Promise<void>((r) => setTimeout(r, 0));

describe("LeaderElection", () => {
  it("elects one leader and hands over in queue order", async () => {
    const locks = new LocalLocks();
    const led: string[] = [];
    const tabs = ["a", "b", "c"].map((id) => {
      const e = new LeaderElection({ locks, onLead: () => led.push(id) });
      e.start();
      return e;
    });
    const [a, b, c] = tabs as [LeaderElection, LeaderElection, LeaderElection];
    await tick();
    expect(led).toEqual(["a"]);
    expect(tabs.map((t) => t.isLeader)).toEqual([true, false, false]);
    expect(locks.names).toEqual([LEADER_LOCK]);

    await a.stop();
    await tick();
    expect(led).toEqual(["a", "b"]);
    expect(tabs.map((t) => t.isLeader)).toEqual([false, true, false]);

    // a follower that leaves the queue never leads
    await c.stop();
    await b.stop();
    await tick();
    expect(led).toEqual(["a", "b"]);
    expect(tabs.map((t) => t.isLeader)).toEqual([false, false, false]);
    expect(locks.names).toEqual([]);
  });

  it("a new tab leads once the lock is free", async () => {
    const locks = new LocalLocks();
    const led: string[] = [];
    const first = new LeaderElection({ locks, onLead: () => led.push("first") });
    first.start();
    await tick();
    await first.stop();
    const second = new LeaderElection({ locks, onLead: () => led.push("second") });
    second.start();
    await tick();
    expect(led).toEqual(["first", "second"]);
    expect(second.isLeader).toBe(true);
    await second.stop();
  });

  it("keeps the lock if starting the tasks throws, and never restarts after stop", async () => {
    const locks = new LocalLocks();
    const e = new LeaderElection({
      locks,
      onLead: () => {
        throw new Error("boom");
      },
    });
    e.start();
    await tick();
    expect(e.isLeader).toBe(true);
    await e.stop();
    e.start();
    await tick();
    expect(e.isLeader).toBe(false);
    expect(locks.names).toEqual([]);
  });
});

/** An in-memory BroadcastChannel: a message reaches every other endpoint, never its sender. */
function hub(): () => ChannelLike & { closed: boolean } {
  const ends = new Set<{ deliver: (data: unknown) => void }>();
  return () => {
    const listeners: ((ev: MessageEvent) => void)[] = [];
    const end = {
      deliver: (data: unknown) => {
        for (const l of listeners) l({ data } as MessageEvent);
      },
    };
    ends.add(end);
    const ch = {
      closed: false,
      postMessage(message: unknown) {
        const copy = structuredClone(message);
        for (const e of ends) if (e !== end) e.deliver(copy);
      },
      addEventListener(_type: "message", l: (ev: MessageEvent) => void) {
        listeners.push(l);
      },
      close() {
        ch.closed = true;
        ends.delete(end);
      },
    };
    return ch;
  };
}

const changed: EngineEvent = { kind: "changed", topics: [{ kind: "forum", uri: "at://f" }] };

describe("ChangeRelay", () => {
  it("relays changed and write events to the other tabs only", () => {
    const open = hub();
    const got: Record<string, [EngineEvent, string][]> = { a: [], b: [], c: [] };
    const relays = Object.fromEntries(
      ["a", "b", "c"].map((id) => [
        id,
        new ChangeRelay({ channel: open(), id, onEvent: (e, from) => got[id]?.push([e, from]) }),
      ]),
    ) as Record<string, ChangeRelay>;
    relays.a?.publish(changed);
    const write: EngineEvent = { kind: "write", id: "w1", state: { kind: "pending" } };
    relays.b?.publish(write);
    // other kinds stay in their tab
    relays.a?.publish({ kind: "syncProgress", forum: "at://f", done: 1, total: 2 });
    expect(got.a).toEqual([[write, "b"]]);
    expect(got.b).toEqual([[changed, "a"]]);
    expect(got.c).toEqual([
      [changed, "a"],
      [write, "b"],
    ]);
  });

  it("ignores its own id, malformed and unknown messages", () => {
    const open = hub();
    const got: EngineEvent[] = [];
    new ChangeRelay({ channel: open(), id: "me", onEvent: (e) => got.push(e) });
    const raw = open();
    for (const m of [
      null,
      "changed",
      { type: "wake", from: "x" },
      { type: "event", from: "me", event: changed },
      { type: "event", event: changed },
      { type: "event", from: "x" },
      { type: "event", from: "x", event: { kind: "changed" } },
      { type: "event", from: "x", event: { kind: "notify" } },
    ]) {
      raw.postMessage(m);
    }
    expect(got).toEqual([]);
    raw.postMessage({ type: "event", from: "x", event: changed });
    expect(got).toEqual([changed]);
  });

  it("closes its channel", () => {
    const ch = hub()();
    new ChangeRelay({ channel: ch, id: "a", onEvent: () => {} }).close();
    expect(ch.closed).toBe(true);
  });
});

describe("host locks", () => {
  it("names Web Locks after the host lock", () => {
    expect(webLockName("outbox:did:plc:abc")).toBe("lessdugin-outbox-did:plc:abc");
    expect(webLockName("sync")).toBe("lessdugin-sync");
  });

  it("excludes holders; ifAvailable answers null; waiting resolves on release", async () => {
    const locks = new LocalLocks();
    const a = await acquireLock(locks, "outbox:did:plc:a", true);
    expect(a).not.toBeNull();
    expect(locks.names).toEqual(["lessdugin-outbox-did:plc:a"]);
    expect(await acquireLock(locks, "outbox:did:plc:a", false)).toBeNull();
    // another name is independent
    const other = await acquireLock(locks, "outbox:did:plc:b", false);
    expect(other).not.toBeNull();
    other?.release();

    let got = false;
    const waiting = acquireLock(locks, "outbox:did:plc:a", true).then((h) => {
      got = true;
      return h;
    });
    await tick();
    expect(got).toBe(false);
    a?.release();
    a?.release(); // idempotent
    const b = await waiting;
    expect(got).toBe(true);
    b?.release();
    await tick();
    expect(locks.names).toEqual([]);
  });

  it("an aborted waiter leaves the queue", async () => {
    const locks = new LocalLocks();
    const held = await acquireLock(locks, "x", true);
    const ctl = new AbortController();
    const ran: string[] = [];
    const p = locks.request("lessdugin-x", { signal: ctl.signal }, () => {
      ran.push("aborted waiter");
    });
    ctl.abort();
    await expect(p).rejects.toMatchObject({ name: "AbortError" });
    held?.release();
    await tick();
    expect(ran).toEqual([]);
    expect(locks.names).toEqual([]);
  });
});
