// SPDX-License-Identifier: AGPL-3.0-or-later
// What a loaded media blob may become in the SPA (sync.md §11.1–§11.2, threat-model SPA-7).
// The engine streams the bytes, checks them against the CID and sniffs their first 4,096 bytes;
// `LoadedMedia.mime` is set only when the sniffed and declared types agree and the type is on
// the display allowlist, and the client types the `blob:` URL with it (else
// application/octet-stream). The SPA shows the blob as an image or a player only when that type
// is also on its own copy of the allowlist and the placeholder is of that kind; everything else,
// SVG and PDF included, is a download card saved as `<cid>.<ext>`.
//
// A download card's URL is application/octet-stream, as SPA-7 asks, except for a display type in
// a placeholder of the other kind (an image type in a `span.media`, whose declared type was not
// written exactly as the table's, lfm-1.md §6.2; a player type in an `img`). That URL keeps the
// allowlist constant: never the declared string, and a type the browser could show inline
// anyway. The card's anchor carries `download`, so the bytes are saved and never opened. The SPA
// cannot re-type the blob: CSP `connect-src` has no `blob:`. A download without a display type is
// named from `LoadedMedia.sniffed`, the type the bytes sniff as whatever was declared (sync.md
// §11.2, engine-api.md §3.4 `load_media`), and `<cid>.bin` when that is unknown too.

import type { EngineApi } from "@lessdugin/engine/api";
import type { Cid, LoadedMedia, MediaView } from "@lessdugin/engine/types";

/** The display allowlist (sync.md §11.2), with the element each type becomes. */
const DISPLAY = {
  "image/png": "img",
  "image/jpeg": "img",
  "image/gif": "img",
  "image/webp": "img",
  "image/avif": "img",
  "video/mp4": "video",
  "video/webm": "video",
  "audio/mp4": "audio",
  "audio/mpeg": "audio",
  "audio/ogg": "audio",
} as const;

const EXT: Record<string, string> = {
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/avif": "avif",
  "video/mp4": "mp4",
  "video/webm": "webm",
  "audio/mp4": "m4a",
  "audio/mpeg": "mp3",
  "audio/ogg": "ogg",
};

/** Fetches the media `cid` of the item `view` for display (a click, or the engine's autoload
 * verdict, is the consent): the view names its author, declared type and size. */
export function loadMediaFor(
  engine: EngineApi,
  cid: string,
  view: MediaView | undefined,
): Promise<LoadedMedia> {
  return engine.loadMedia(cid as Cid, view);
}

/** The notice of a placeholder that waits for a click (sync.md §11.3): the host that serves
 * the bytes, from the view's `pds`, learns the reader's address. */
export function mediaHostNotice(view: MediaView | undefined): string {
  let host = "";
  try {
    host = view ? new URL(view.pds).host : "";
  } catch {
    host = "";
  }
  return host !== ""
    ? `loads from ${host}, which will see your IP`
    : "the author's server will see your IP";
}

/** `filename` is what a download card saves: `<cid>.<ext>`, `bin` when the type is unknown. */
export type MediaDisplay =
  | { kind: "img" | "video" | "audio"; filename: string }
  | { kind: "download"; filename: string };

/** The extension a download card names its file with, by the type the bytes sniff as
 * (data-model.md §8.8's table, or `image/avif`; a type the SPA never shows inline is a download
 * too). Nothing but these constants reaches the file name. */
const SNIFFED_EXT: Record<string, string> = {
  ...EXT,
  "image/svg+xml": "svg",
  "application/pdf": "pdf",
  "text/markdown": "md",
};

/** What `loaded` may become in a placeholder of kind `placeholder` (sync.md §11.1: else
 * "download card only"). */
export function mediaDisplay(loaded: LoadedMedia, placeholder: "img" | "span"): MediaDisplay {
  // a CID is base32 (lowercase letters and 2–7): nothing else reaches the file name
  const name = /^b[a-z2-7]{8,100}$/.test(loaded.cid) ? loaded.cid : "media";
  const mime = loaded.mime ?? "";
  const kind = Object.hasOwn(DISPLAY, mime) ? DISPLAY[mime as keyof typeof DISPLAY] : null;
  const sniffed = loaded.sniffed ?? "";
  const ext =
    (kind !== null && EXT[mime]) ||
    (Object.hasOwn(SNIFFED_EXT, sniffed) && SNIFFED_EXT[sniffed]) ||
    "bin";
  const filename = `${name}.${ext}`;
  if (kind !== null && (kind === "img") === (placeholder === "img")) return { kind, filename };
  return { kind: "download", filename };
}
