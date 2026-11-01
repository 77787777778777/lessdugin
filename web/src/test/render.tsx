// SPDX-License-Identifier: AGPL-3.0-or-later
// Test helpers: render a component inside the app's providers (engine, query client, public-
// action interstitials, toasts) and a memory-history router.

import { MockEngine, type MockEngineOptions } from "@lessdugin/engine/mock";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { PublicAckProvider } from "../components/PublicAck.tsx";
import { ToastRegion } from "../components/Toast.tsx";
import { EngineProvider } from "../engine/context.tsx";
import { createMockHandle } from "../engine/mock.ts";
import { invalidateTopics } from "../engine/queries.ts";
import { writeStore } from "../engine/writes.ts";

export function renderWithApp(
  ui: ReactNode,
  opts: {
    engine?: MockEngine;
    engineOptions?: MockEngineOptions;
    /** Render `ui` as the component of this route (a path with params, as router.tsx declares
     * it), with the history at `at`: for a page that reads its params through `getRouteApi`. */
    route?: { path: string; at: string };
  } = {},
) {
  const engine = opts.engine ?? new MockEngine(opts.engineOptions);
  const handle = createMockHandle(engine);
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const unsubscribe = engine.subscribe((e) => {
    if (e.kind === "changed") invalidateTopics(queryClient, e.topics);
    if (e.kind === "write") writeStore.update(e.id, e.state);
  });
  const rootRoute = createRootRoute({
    component: () => (
      <>
        {opts.route ? <Outlet /> : ui}
        <ToastRegion />
      </>
    ),
  });
  const routeTree = opts.route
    ? rootRoute.addChildren([
        createRoute({
          getParentRoute: () => rootRoute,
          path: opts.route.path,
          component: () => ui,
        }),
      ])
    : rootRoute;
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [opts.route?.at ?? "/"] }),
  });
  const result = render(
    <EngineProvider engine={handle}>
      <QueryClientProvider client={queryClient}>
        <PublicAckProvider>
          <RouterProvider router={router as never} />
        </PublicAckProvider>
      </QueryClientProvider>
    </EngineProvider>,
  );
  return { ...result, engine, queryClient, router, unsubscribe };
}
