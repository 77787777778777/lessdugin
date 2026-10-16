// SPDX-License-Identifier: AGPL-3.0-or-later
// Per-viewer conveniences only (a remembered forum, panel state). Storage can be unavailable
// (private windows, blocked site data), so every access is guarded and has a fallback.

const memory = new Map<string, string>();

export function readLocal(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return memory.get(key) ?? null;
  }
}

export function writeLocal(key: string, value: string | null): void {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    if (value === null) memory.delete(key);
    else memory.set(key, value);
  }
}
