// SPDX-License-Identifier: AGPL-3.0-or-later
import type { EngineApi } from "@lessdugin/engine/api";
import { createContext, type ReactNode, use } from "react";
import type { EngineHandle } from "./handle.ts";

const EngineContext = createContext<EngineHandle | null>(null);

export function EngineProvider({
  engine,
  children,
}: {
  engine: EngineHandle;
  children: ReactNode;
}) {
  return <EngineContext value={engine}>{children}</EngineContext>;
}

export function useEngineHandle(): EngineHandle {
  const e = use(EngineContext);
  if (!e) throw new Error("useEngineHandle outside EngineProvider");
  return e;
}

export function useEngine(): EngineApi {
  return useEngineHandle().api;
}
