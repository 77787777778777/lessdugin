// SPDX-License-Identifier: AGPL-3.0-or-later
// An edit draft may combine raw and bodySource only when both name the same record revision.
import type { BodySource, RawView } from "@lessdugin/engine/types";
import { describe, expect, it } from "vitest";
import { coherentEditSource } from "./editSource.ts";

const URI = "at://did:plc:abcdefghijklmnopqrstuvwx/x.y.post/r";
const raw = (cid: string, markdown: string, mediaCid?: string): RawView => ({
  dto: 1,
  uri: URI,
  cid,
  level: "pds",
  record: {
    body: {
      text: markdown,
      ...(mediaCid === undefined
        ? {}
        : {
            media: [
              {
                blob: {
                  $type: "blob",
                  ref: { $link: mediaCid },
                  mimeType: "image/png",
                  size: 4,
                },
                alt: "revision-specific media",
              },
            ],
          }),
    },
  },
  validity: [],
  assertions: [],
});
const source = (cid: string, markdown: string): BodySource => ({
  cid,
  markdown,
  profile: "lfm/1",
  spilled: true,
});

describe("coherentEditSource", () => {
  it("refuses to combine one revision's source with another revision's raw fields", async () => {
    const oldRaw = raw("record-old", "old inline", "media-old");
    const newRaw = raw("record-new", "new inline", "media-new");
    const oldSource = source("record-old", "old spilled");
    const newSource = source("record-new", "new spilled");
    const engine = {
      raw: async () => oldRaw,
      bodySource: async () => newSource,
    };
    await expect(coherentEditSource(engine, URI)).rejects.toMatchObject({
      code: "conflict",
      retryable: true,
      detail: { kind: "revisionChanged" },
    });
    // The converse ordering is also rejected: no newest-fields substitution is allowed.
    await expect(
      coherentEditSource({ raw: async () => newRaw, bodySource: async () => oldSource }, URI),
    ).rejects.toMatchObject({ detail: { kind: "revisionChanged" } });
  });

  it("returns one revision's record and source after a delayed coherent pair", async () => {
    const record = raw("record-current", "ignored inline", "media-current");
    const body = source("record-current", "actual spilled source");
    const result = await coherentEditSource(
      {
        raw: () => new Promise((resolve) => setTimeout(() => resolve(record), 2)),
        bodySource: async () => body,
      },
      URI,
    );
    expect(result.raw).toBe(record);
    expect(result.source).toBe(body);
    expect(result.source.markdown).toBe("actual spilled source");
    expect(result.raw.cid).toBe(result.source.cid);
  });
});
