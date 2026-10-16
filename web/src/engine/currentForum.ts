// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum the user last opened: the context for user pages, search and `handle/rkey` refs.
// Read lazily, after main.tsx has chosen the storage namespace for the engine in use.

import { useSyncExternalStore } from "react";
import { readLocal, writeLocal } from "../lib/storage.ts";

const KEY = "currentForum";
const listeners = new Set<() => void>();
let current: string | null | undefined;

function get(): string | null {
  if (current === undefined) current = readLocal(KEY);
  return current;
}

export function setCurrentForum(uri: string): void {
  if (get() === uri) return;
  current = uri;
  writeLocal(KEY, uri);
  for (const l of [...listeners]) l();
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

export function useCurrentForum(): string | null {
  return useSyncExternalStore(subscribe, get, get);
}
