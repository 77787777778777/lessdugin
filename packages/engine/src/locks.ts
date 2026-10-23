// SPDX-License-Identifier: AGPL-3.0-or-later
//
// `Host::lock(name, wait)` in the browser (sync.md §10.6, §12.3): the Web Lock
// `lessdugin-<name>` with the first `:` replaced by `-` (`outbox:<did>` →
// `lessdugin-outbox-<did>`, the DID unchanged), exclusive, `ifAvailable` unless `wait`. Web Locks
// exclude other tabs of the origin and other tasks of this worker alike, and are released when
// the holder's tab or worker goes away, so a lock is never left stale. The handle is released
// when the Rust guard drops (`release()`).

/** A held lock. */
export interface LockHandle {
  release(): void;
}

/** The Web Lock name of a host lock name. */
export function webLockName(name: string): string {
  return `lessdugin-${name.replace(":", "-")}`;
}

/** The part of `LockManager` the host uses (injectable for tests). */
export interface LockManagerLike {
  request(
    name: string,
    options: LockOptions,
    callback: (lock: unknown) => Promise<void> | void,
  ): Promise<unknown>;
}

export interface LockOptions {
  mode?: "exclusive" | "shared";
  ifAvailable?: boolean;
  /** Aborting it leaves the queue (the request rejects with an `AbortError`). */
  signal?: AbortSignal;
}

/** Takes the host lock `name`; `null` when `!wait` and another holder has it. */
export function acquireLock(
  locks: LockManagerLike,
  name: string,
  wait: boolean,
): Promise<LockHandle | null> {
  return new Promise<LockHandle | null>((resolve, reject) => {
    locks
      .request(webLockName(name), { mode: "exclusive", ifAvailable: !wait }, (lock) => {
        if (lock === null) {
          resolve(null);
          return;
        }
        // held until the guard releases it
        return new Promise<void>((release) => {
          let released = false;
          resolve({
            release() {
              if (!released) {
                released = true;
                release();
              }
            },
          });
        });
      })
      .catch(reject);
  });
}

/** A process-local `LockManager` (tests, and environments without Web Locks): exclusive,
 * FIFO, `ifAvailable` supported. It excludes holders within this JS realm only. */
export class LocalLocks implements LockManagerLike {
  private readonly held = new Set<string>();
  private readonly queues = new Map<string, { go: () => void }[]>();

  /** Names currently held. */
  get names(): string[] {
    return [...this.held];
  }

  async request(
    name: string,
    options: LockOptions,
    callback: (lock: unknown) => Promise<void> | void,
  ): Promise<unknown> {
    if (options.signal?.aborted) throw new DOMException("aborted", "AbortError");
    if (this.held.has(name)) {
      if (options.ifAvailable) return callback(null);
      await new Promise<void>((go, reject) => {
        const q = this.queues.get(name) ?? [];
        const entry = { go };
        q.push(entry);
        this.queues.set(name, q);
        options.signal?.addEventListener("abort", () => {
          const i = q.indexOf(entry);
          if (i >= 0) {
            q.splice(i, 1);
            reject(new DOMException("aborted", "AbortError"));
          }
        });
      });
    }
    this.held.add(name);
    try {
      return await callback({ name, mode: "exclusive" });
    } finally {
      const next = this.queues.get(name)?.shift();
      if (next) next.go();
      else this.held.delete(name);
    }
  }
}
