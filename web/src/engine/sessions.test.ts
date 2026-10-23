// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { EndedSessions, SESSION_ENDED_MESSAGE } from "./sessions.ts";

describe("EndedSessions", () => {
  it("tells the user once per ended session, until a sign-in clears it", () => {
    const told: string[] = [];
    const s = new EndedSessions((m) => told.push(m));
    const seen: number[] = [];
    s.subscribe(() => seen.push(s.snapshot()));
    s.end("did:plc:a");
    s.end("did:plc:a");
    expect(s.has("did:plc:a")).toBe(true);
    expect(told).toEqual([SESSION_ENDED_MESSAGE]);
    s.clear("did:plc:a");
    expect(s.has("did:plc:a")).toBe(false);
    s.clear("did:plc:a");
    s.end("did:plc:a");
    expect(told).toHaveLength(2);
    expect(seen).toHaveLength(3);
  });
});
