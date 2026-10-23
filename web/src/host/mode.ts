// SPDX-License-Identifier: AGPL-3.0-or-later
// Which host serves this copy of the SPA (synthesis §6.13): `lessdugin-config.json` next to
// `index.html` says `{"oauth": "hosted"}` on the static site (the file ships in the build) and
// `{"oauth": "bridge"}` when `lessdugin web` serves it (the bridge answers that path itself,
// threat-model WEB-5). Anything unreadable means hosted: the bridge always answers.

export type HostMode = "hosted" | "bridge";

/** Parses the body of `lessdugin-config.json`. */
export function parseHostConfig(text: string): HostMode {
  try {
    const v = JSON.parse(text) as { oauth?: unknown };
    return v.oauth === "bridge" ? "bridge" : "hosted";
  } catch {
    return "hosted";
  }
}

/** Fetches `lessdugin-config.json` from the page's own origin and directory. */
export async function loadHostMode(
  base: string = document.baseURI,
  fetchFn: typeof fetch = globalThis.fetch.bind(globalThis),
): Promise<HostMode> {
  try {
    const resp = await fetchFn(new URL("lessdugin-config.json", base).href, {
      cache: "no-cache",
      credentials: "omit",
      redirect: "error",
    });
    if (!resp.ok) return "hosted";
    return parseHostConfig(await resp.text());
  } catch {
    return "hosted";
  }
}
