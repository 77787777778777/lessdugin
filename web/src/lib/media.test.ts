// SPDX-License-Identifier: AGPL-3.0-or-later
// Display decisions for loaded media (sync.md §11.2, threat-model SPA-7).
import { describe, expect, it } from "vitest";
import { mediaDisplay } from "./media.ts";

const CID = "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy";
const loaded = (mime?: string, cid = CID) => ({
  cid,
  size: 1,
  url: "blob:t/1",
  ...(mime === undefined ? {} : { mime }),
});

describe("mediaDisplay (sync.md §11.2)", () => {
  it("shows the display types in a placeholder of their kind", () => {
    expect(mediaDisplay(loaded("image/png"), "img")).toEqual({
      kind: "img",
      filename: `${CID}.png`,
    });
    expect(mediaDisplay(loaded("image/avif"), "img").kind).toBe("img");
    expect(mediaDisplay(loaded("video/webm"), "span").kind).toBe("video");
    expect(mediaDisplay(loaded("audio/mp4"), "span")).toEqual({
      kind: "audio",
      filename: `${CID}.m4a`,
    });
  });

  it("offers everything else as a download named after the CID", () => {
    // no display type: the sniffed and declared types disagree, or the type is not shown
    expect(mediaDisplay(loaded(), "img")).toEqual({ kind: "download", filename: `${CID}.bin` });
    expect(mediaDisplay(loaded(), "span").kind).toBe("download");
    // a type the SPA's allowlist does not have is never shown, whatever the engine says
    expect(mediaDisplay(loaded("image/svg+xml"), "img")).toEqual({
      kind: "download",
      filename: `${CID}.bin`,
    });
    expect(mediaDisplay(loaded("toString"), "img").kind).toBe("download");
    // a player never replaces an image, nor an image a player's placeholder
    expect(mediaDisplay(loaded("video/mp4"), "img")).toEqual({
      kind: "download",
      filename: `${CID}.mp4`,
    });
    expect(mediaDisplay(loaded("image/png"), "span").kind).toBe("download");
    // only a CID reaches the file name
    expect(mediaDisplay(loaded("image/png", "../x"), "span").filename).toBe("media.png");
  });
});
