// SPDX-License-Identifier: AGPL-3.0-or-later
// threat-model.md SPA-8: the OAuth callback (`response_mode=query`, AUTH-5) and, in `web` mode,
// the bridge capability (§7.2, `#lessdugin-cap=…`) are read and removed with
// `history.replaceState` before the hash router mounts, so they never reach a route, a link the
// router creates, the history stack or a later referrer-free navigation. The values are kept
// here, in memory only, for the login code to consume once.

/** The query parameters of an OAuth authorization response (RFC 6749 §4.1.2, RFC 9207). */
const OAUTH_KEYS = ["code", "state", "iss", "error", "error_description", "error_uri"] as const;

export interface BootParams {
  /** The OAuth callback's parameters, when the page was opened as the redirect target. */
  oauth: Partial<Record<(typeof OAUTH_KEYS)[number], string>> | null;
  /** The `lessdugin web` bridge capability (43 base64url characters), when present. */
  bridgeCapability: string | null;
}

/** Splits the boot parameters off a location; returns the cleaned `search` and `hash`. */
export function splitBootParams(loc: { search: string; hash: string }): {
  params: BootParams;
  search: string;
  hash: string;
} {
  let search = loc.search;
  let hash = loc.hash;
  let oauth: BootParams["oauth"] = null;
  const sp = new URLSearchParams(search);
  if (sp.has("state") && (sp.has("code") || sp.has("error"))) {
    oauth = {};
    for (const k of OAUTH_KEYS) {
      const v = sp.get(k);
      if (v !== null) oauth[k] = v;
      sp.delete(k);
    }
    const rest = sp.toString();
    search = rest === "" ? "" : `?${rest}`;
  }
  let bridgeCapability: string | null = null;
  if (hash.startsWith("#lessdugin-cap=")) {
    const cap = hash.slice("#lessdugin-cap=".length);
    bridgeCapability = /^[A-Za-z0-9_-]{43}$/.test(cap) ? cap : null;
    hash = "#/";
  }
  return { params: { oauth, bridgeCapability }, search, hash };
}

let taken: BootParams = { oauth: null, bridgeCapability: null };

/** Removes the boot parameters from the address bar (call before the router mounts). */
export function takeBootParams(win: Window = window): BootParams {
  const { params, search, hash } = splitBootParams(win.location);
  if (search !== win.location.search || hash !== win.location.hash) {
    win.history.replaceState(win.history.state, "", `${win.location.pathname}${search}${hash}`);
  }
  taken = params;
  return params;
}

/** The OAuth callback parameters, once (the login code consumes them). */
export function consumeOAuthCallback(): BootParams["oauth"] {
  const out = taken.oauth;
  taken = { ...taken, oauth: null };
  return out;
}

/** The bridge capability, once (exchanged for a session by the `web` bridge client). */
export function consumeBridgeCapability(): string | null {
  const out = taken.bridgeCapability;
  taken = { ...taken, bridgeCapability: null };
  return out;
}
