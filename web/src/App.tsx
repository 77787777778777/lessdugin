// SPDX-License-Identifier: AGPL-3.0-or-later
import { isEngineError } from "@lessdugin/engine/types";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { PublicAckProvider } from "./components/PublicAck.tsx";
import { toasts } from "./components/Toast.tsx";
import { EngineProvider } from "./engine/context.tsx";
import type { EngineHandle } from "./engine/handle.ts";
import { invalidateTopics } from "./engine/queries.ts";
import { startEngineTasks, syncProgress } from "./engine/sync.ts";
import { writeStore } from "./engine/writes.ts";
import { createAppRouter } from "./router.tsx";

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 30_000,
        refetchOnWindowFocus: false,
        retry: (count, error) => (isEngineError(error) ? error.retryable && count < 2 : count < 1),
      },
    },
  });
}

export function App({ engine }: { engine: EngineHandle }) {
  const [queryClient] = useState(createQueryClient);
  const [router] = useState(createAppRouter);

  // Engine events: `changed` topics invalidate queries (UIs re-query, engine-api.md §3.8),
  // `write` events drive write tickets, warnings surface as toasts.
  useEffect(
    () =>
      engine.api.subscribe((event) => {
        switch (event.kind) {
          case "changed":
            invalidateTopics(queryClient, event.topics);
            break;
          case "write":
            writeStore.update(event.id, event.state);
            break;
          case "warning":
            toasts.show({ message: event.message, tone: "error" });
            break;
          case "auth":
            if (event.state.kind === "expiring") {
              toasts.show({ message: "Your session expires soon; sign in again to keep posting." });
            } else if (event.state.kind === "expired") {
              toasts.show({ message: "Your session has expired. Sign in again.", tone: "error" });
            }
            break;
          case "syncProgress":
            syncProgress.update(event.forum, event.done, event.total);
            break;
        }
      }),
    [engine, queryClient],
  );

  // The engine's long-lived tasks (live tail, outbox pump) run for the app's lifetime.
  useEffect(() => startEngineTasks(engine.api), [engine]);

  return (
    <EngineProvider engine={engine}>
      <QueryClientProvider client={queryClient}>
        <PublicAckProvider>
          <RouterProvider router={router} />
        </PublicAckProvider>
      </QueryClientProvider>
    </EngineProvider>
  );
}
