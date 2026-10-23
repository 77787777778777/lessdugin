// SPDX-License-Identifier: AGPL-3.0-or-later
//
// One leader tab per origin (sync.md §10.6). Every engine worker requests the Web Lock
// `lessdugin-sync-leader` once and holds it for its lifetime; the holder is the leader and the
// only worker that runs the long-lived `outboxPump` and `liveTail`. When the leader's tab closes
// (or its worker stops), the browser releases the lock and the next waiting worker takes over,
// resuming from the persisted outbox and cursors.
//
// Followers still read the shared IndexedDB stores and enqueue writes into the shared outbox.
// Every worker relays its engine's `changed` and `write` events on the BroadcastChannel
// `lessdugin-changes`; a receiving worker drops the cached models the topics touch
// (`externalChange`) and forwards the event to its own subscribers, and the leader wakes its
// pump when another tab queued a write (a `pending` write event).

import type { LockManagerLike } from "./locks.ts";
import { CHANGES_CHANNEL, type ChannelMessage, LEADER_LOCK } from "./protocol.ts";
import type { EngineEvent } from "./types.ts";

export interface LeaderOptions {
  locks: LockManagerLike;
  /** Called once, when this worker becomes the leader. */
  onLead: () => void;
  /** The lock name (default `lessdugin-sync-leader`). */
  name?: string;
}

/** The leader election: `start()` queues for the lock; `stop()` leaves the queue or hands the
 * leadership over. */
export class LeaderElection {
  private leading = false;
  private stopped = false;
  private release: (() => void) | null = null;
  private readonly abort = new AbortController();
  /** Settles when the lock request ends (released, aborted or failed). */
  done: Promise<void> = Promise.resolve();

  constructor(private readonly opts: LeaderOptions) {}

  get isLeader(): boolean {
    return this.leading;
  }

  start(): void {
    if (this.stopped) return;
    const name = this.opts.name ?? LEADER_LOCK;
    this.done = this.opts.locks
      .request(name, { mode: "exclusive", signal: this.abort.signal }, () => {
        if (this.stopped) return;
        this.leading = true;
        const held = new Promise<void>((resolve) => {
          this.release = resolve;
        });
        try {
          this.opts.onLead();
        } catch {
          // the leader keeps the lock even if starting a task failed; the tasks retry
        }
        return held;
      })
      .then(
        () => undefined,
        () => undefined,
      )
      .finally(() => {
        this.leading = false;
      });
  }

  /** Stops leading (releasing the lock to the next tab) or stops waiting. */
  stop(): Promise<void> {
    this.stopped = true;
    if (this.release) {
      this.release();
      this.release = null;
    } else {
      this.abort.abort();
    }
    return this.done;
  }
}

/** The part of `BroadcastChannel` the relay uses. */
export interface ChannelLike {
  postMessage(message: unknown): void;
  addEventListener(type: "message", listener: (ev: MessageEvent) => void): void;
  close(): void;
}

export interface RelayOptions {
  channel: ChannelLike;
  /** This worker's id (messages it sent are ignored). */
  id: string;
  /** A `changed` or `write` event of another tab. */
  onEvent: (event: EngineEvent, from: string) => void;
}

/** Relays engine events between the tabs' workers. */
export class ChangeRelay {
  constructor(private readonly opts: RelayOptions) {
    opts.channel.addEventListener("message", (ev) => this.receive(ev.data));
  }

  static open(id: string, onEvent: RelayOptions["onEvent"]): ChangeRelay | null {
    if (typeof BroadcastChannel === "undefined") return null;
    return new ChangeRelay({ channel: new BroadcastChannel(CHANGES_CHANNEL), id, onEvent });
  }

  /** Relays a local event (only `changed` and `write` events cross tabs). */
  publish(event: EngineEvent): void {
    if (event.kind !== "changed" && event.kind !== "write") return;
    const m: ChannelMessage = { type: "event", from: this.opts.id, event };
    this.opts.channel.postMessage(m);
  }

  private receive(data: unknown): void {
    const m = data as Partial<ChannelMessage> | null;
    if (m?.type !== "event" || typeof m.from !== "string" || m.from === this.opts.id) return;
    const e = (m as { event?: unknown }).event as EngineEvent | undefined;
    if (!e || (e.kind !== "changed" && e.kind !== "write")) return;
    if (e.kind === "changed" && !Array.isArray(e.topics)) return;
    this.opts.onEvent(e, m.from);
  }

  close(): void {
    this.opts.channel.close();
  }
}
