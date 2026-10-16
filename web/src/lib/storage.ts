// SPDX-License-Identifier: AGPL-3.0-or-later
// Per-viewer conveniences only (a remembered forum, panel state). Storage can be unavailable
// (private windows, blocked site data), so every access is guarded and has a fallback.
//
// Keys are namespaced by engine: the demo (mock) engine's fixture state never lands under the
// keys the real engine reads (`setStorageNamespace` at startup, before anything is read).

const memory = new Map<string, string>();
let namespace = "lessdugin.";

/** Selects the key namespace: `lessdugin.` for the real engine, `lessdugin.mock.` for the
 * demo engine. */
export function setStorageNamespace(kind: "mock" | "wasm"): void {
  namespace = kind === "mock" ? "lessdugin.mock." : "lessdugin.";
}

/** The full storage key for `name` in the current namespace. */
export function storageKey(name: string): string {
  return `${namespace}${name}`;
}

export function readLocal(name: string): string | null {
  const key = storageKey(name);
  try {
    return window.localStorage.getItem(key);
  } catch {
    return memory.get(key) ?? null;
  }
}

export function writeLocal(name: string, value: string | null): void {
  const key = storageKey(name);
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    if (value === null) memory.delete(key);
    else memory.set(key, value);
  }
}
