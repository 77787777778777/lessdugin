// SPDX-License-Identifier: AGPL-3.0-or-later
// Media sources (threat-model SPA-7). The only place allowed to set a `src` through the DOM
// (scripts/check-no-raw-href.mjs). It takes nothing but a `blob:` object URL: the one the
// engine client made from a CID-verified blob, typed with the display type (lib/media.ts). The
// caller owns the URL and revokes it when the element goes away.

/** Points `el` at the object URL `url`; refuses any other URL. */
export function setMediaSource(el: HTMLImageElement | HTMLMediaElement, url: string): void {
  if (!url.startsWith("blob:")) throw new Error("media is shown only from a verified object URL");
  el.src = url;
}
