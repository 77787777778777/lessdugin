// SPDX-License-Identifier: AGPL-3.0-or-later
// Display decisions for loaded media (sync.md §11.2, threat-model SPA-7).
import { describe, expect, it } from "vitest";
import { mediaDisplay } from "./media.ts";

const CID = "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy";
const loaded = (mime?: string, cid = CID, sniffed?: string) => ({
  cid,
  size: 1,
  url: "blob:t/1",
  ...(mime === undefined ? {} : { mime }),
  ...(sniffed === undefined ? {} : { sniffed }),
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

  it("names a download from the sniffed type when there is no display type (m4-3)", () => {
    // a PDF and a spilled text have no display type: the extension comes from the sniff
    expect(mediaDisplay(loaded(undefined, CID, "application/pdf"), "span")).toEqual({
      kind: "download",
      filename: `${CID}.pdf`,
    });
    expect(mediaDisplay(loaded(undefined, CID, "text/markdown"), "img").filename).toBe(`${CID}.md`);
    // declared as an image, sniffed as audio: no display type, but the bytes are an mp3
    expect(mediaDisplay(loaded(undefined, CID, "audio/mpeg"), "img").filename).toBe(`${CID}.mp3`);
    expect(mediaDisplay(loaded(undefined, CID, "image/svg+xml"), "img").filename).toBe(
      `${CID}.svg`,
    );
    // an unknown or hostile sniffed string never reaches the file name
    expect(mediaDisplay(loaded(undefined, CID, "application/x-thing"), "img").filename).toBe(
      `${CID}.bin`,
    );
    expect(mediaDisplay(loaded(undefined, CID, "../../etc"), "img").filename).toBe(`${CID}.bin`);
    expect(mediaDisplay(loaded(undefined, CID, "toString"), "img").filename).toBe(`${CID}.bin`);
    // the display type wins when it is there (a video in an image placeholder)
    expect(mediaDisplay(loaded("video/mp4", CID, "video/mp4"), "img").filename).toBe(`${CID}.mp4`);
  });
});
