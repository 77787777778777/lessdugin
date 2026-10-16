// SPDX-License-Identifier: AGPL-3.0-or-later
// Saving bytes the app produced itself (an export of local state) as a file. The only place
// allowed to set an `href` through the DOM (scripts/check-no-raw-href.mjs): the URL is always
// an object URL this function creates for `blob`, revoked right after the click.

export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.download = filename;
  a.href = url;
  a.rel = "noopener";
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
