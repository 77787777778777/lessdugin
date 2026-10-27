// SPDX-License-Identifier: AGPL-3.0-or-later
// What a loaded media blob may become in the SPA (sync.md §11.1–§11.2, threat-model SPA-7).
// The engine streams the bytes, checks them against the CID and sniffs their first 4,096 bytes;
// `LoadedMedia.mime` is set only when the sniffed and declared types agree and the type is on
// the display allowlist, and the client types the `blob:` URL with it (else
// application/octet-stream). The SPA shows the blob as an image or a player only when that type
// is also on its own copy of the allowlist and the placeholder is of that kind; everything else,
// SVG and PDF included, is a download card saved as `<cid>.<ext>`.

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

/** `LoadedMedia` with its display type (engine-api.md §3.4 `load_media`). */
export type LoadedForDisplay = LoadedMedia & { mime?: string };

/** `loadMedia(cid, media?)`: the item's view names its author, declared type and size. */
type LoadMedia = (cid: Cid, media?: MediaView) => Promise<LoadedForDisplay>;

/** Fetches the media `cid` of the item `view` for display (a click is the consent). */
export function loadMediaFor(
  engine: EngineApi,
  cid: string,
  view: MediaView | undefined,
): Promise<LoadedForDisplay> {
  return (engine.loadMedia as LoadMedia).call(engine, cid as Cid, view);
}

/** `filename` is what a download card saves: `<cid>.<ext>`, `bin` when the type is unknown. */
export type MediaDisplay =
  | { kind: "img" | "video" | "audio"; filename: string }
  | { kind: "download"; filename: string };

/** What `loaded` may become in a placeholder of kind `placeholder` (sync.md §11.1: else
 * "download card only"). */
export function mediaDisplay(loaded: LoadedForDisplay, placeholder: "img" | "span"): MediaDisplay {
  // a CID is base32 (lowercase letters and 2–7): nothing else reaches the file name
  const name = /^b[a-z2-7]{8,100}$/.test(loaded.cid) ? loaded.cid : "media";
  const mime = loaded.mime ?? "";
  const kind = Object.hasOwn(DISPLAY, mime) ? DISPLAY[mime as keyof typeof DISPLAY] : null;
  const filename = `${name}.${(kind !== null && EXT[mime]) || "bin"}`;
  if (kind !== null && (kind === "img") === (placeholder === "img")) return { kind, filename };
  return { kind: "download", filename };
}
