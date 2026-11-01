// SPDX-License-Identifier: AGPL-3.0-or-later
// raw(uri) and bodySource(uri) are separate engine reads. Only their equal record CIDs make
// their fields one revision an edit may use (engine-api.md §3.4 "Body source revision").

import type { EngineApi } from "@lessdugin/engine/api";
import type { BodySource, RawView } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { isSpilledBody, objectField } from "../lib/records.ts";
import { qk } from "./queries.ts";

export type EngineSource = Pick<EngineApi, "raw" | "bodySource">;
export interface CoherentEditSource {
  raw: RawView;
  source: BodySource;
}

/** A failed source read retains the raw revision solely for the unavailable-source form; it
 * never becomes a draft without a later CID-coherent pair. */
export class SpilledBodySourceError extends Error {
  constructor(
    readonly raw: RawView,
    readonly sourceError: unknown,
  ) {
    const message =
      typeof sourceError === "object" && sourceError !== null && "message" in sourceError
        ? String(sourceError.message)
        : String(sourceError);
    super(`The body's text is stored in a blob that could not be loaded (${message}). Try again.`);
  }
}

/** Reads both operations for this attempt, never an independently cached half. A mismatch is
 * retryable and yields no draft: newer media/link/card fields are never substituted for the
 * revision whose markdown was returned. */
export async function coherentEditSource(
  engine: EngineSource,
  uri: string,
): Promise<CoherentEditSource> {
  const raw = await engine.raw(uri);
  const source = await engine.bodySource(uri).catch((error: unknown) => {
    if (
      isSpilledBody(objectField(raw.record, "body")) ||
      isSpilledBody(objectField(raw.record, "about"))
    ) {
      throw new SpilledBodySourceError(raw, error);
    }
    throw error;
  });
  if (raw.cid === undefined || raw.cid !== source.cid) {
    throw {
      code: "conflict",
      message:
        "The record changed while its edit source was loading. Try again to edit one revision.",
      retryable: true,
      detail: { kind: "revisionChanged" },
    };
  }
  return { raw, source };
}

/** The one edit-source query for forum, post and comment forms. CID is explicit in the key,
 * and there is no freshness window in which two independent source queries can be combined. */
export function useCoherentEditSource(engine: EngineSource, uri: string, enabled = true) {
  const qc = useQueryClient();
  const [attempt, setAttempt] = useState(0);
  const query = useQuery({
    queryKey: qk.editSource(uri, attempt),
    queryFn: () => coherentEditSource(engine, uri),
    enabled,
    staleTime: 0,
    gcTime: 0,
    retry: false,
  });
  const retry = (): void => {
    qc.removeQueries({ queryKey: qk.editSource(uri), exact: false });
    setAttempt((n) => n + 1);
  };
  return { ...query, retry };
}
