// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum the user last opened: the context for user pages, search and `handle/rkey` refs.

import { useSyncExternalStore } from "react";
import { readLocal, writeLocal } from "../lib/storage.ts";

const KEY = "lessdugin.currentForum";
const listeners = new Set<() => void>();
let current: string | null = readLocal(KEY);

export function setCurrentForum(uri: string): void {
  if (current === uri) return;
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
  return useSyncExternalStore(
    subscribe,
    () => current,
    () => current,
  );
}
