// SPDX-License-Identifier: AGPL-3.0-or-later
// Saving bytes as a file: ones the app produced itself (an export of local state), or a media
// blob the engine verified (a download card, threat-model SPA-7). The only place allowed to set
// an `href` through the DOM (scripts/check-no-raw-href.mjs): the URL is always an object URL.

export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  save(url, filename);
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Saves the object URL `url` (its owner revokes it); refuses any other URL. */
export function downloadObjectUrl(url: string, filename: string): void {
  if (!url.startsWith("blob:")) throw new Error("only an object URL is saved");
  save(url, filename);
}

function save(url: string, filename: string): void {
  const a = document.createElement("a");
  a.download = filename;
  a.href = url;
  a.rel = "noopener";
  a.click();
}
