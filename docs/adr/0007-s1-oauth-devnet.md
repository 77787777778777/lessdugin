<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0007 — S1: native OAuth uses jacquard-oauth's building blocks behind a lessdugin facade

- Status: Accepted, pending user run
- Date: 2026-10-16
- Spikes: S1
- Amends: none (implements `docs/design/synthesis-v1.md` §6.7; `data-model.md` §10;
  `sync.md` §3.6; `threat-model.md` AUTH-1…AUTH-11)

## Context

Synthesis §6.7 puts native authentication behind the `AuthSession` facade and names
`jacquard-oauth` `=0.12.2` for OAuth (loopback client, hosted native metadata, `--no-browser`).
If S1 failed, the fallback was our own PAR + PKCE + DPoP client, about 1.5k lines over atp's
XRPC. The normative requirements that a library has to fit:

- all HTTP through the native SSRF guard (`Net`), no redirects for `UserPds`;
- `response_mode=query`, `state` and PKCE checked on every path (AUTH-5);
- the loopback `client_id` exactly as in `data-model.md` §10.4, and the hosted metadata after
  `invalid_client` or with `--oauth-client hosted`;
- the scope fallback decided at PAR time, never `transition:generic` (AUTH-1, AUTH-2);
- the login binding of `sync.md` §3.6 enforced by the core, with the issuer pinned on first use;
- sessions in the OS keyring or a 0600 file, refreshed under an exclusive lock (AUTH-7).

## Method

- Devnet: `packages/e2e/devnet.mjs` (`@atproto/dev-env` 0.6.20, `TestNetworkNoAppView`). The
  PDS is its own authorization server, issuer `http://localhost:<port>`, PAR required, DPoP
  required, `authorization_response_iss_parameter_supported: true`.
- Browser: headless Chromium driven by Playwright 1.56.1
  (`crates/cli/tests/support/oauth_approve.mjs`): it opens the printed authorization URL, signs
  in with the account's password, presses **Authorize**, and either records the failed redirect
  to `http://127.0.0.1:<port>/callback` (paste-back) or lets it reach the CLI's listener.
- Test: `crates/cli/tests/e2e_devnet.rs` (run with `LESSDUGIN_E2E=1`), which starts and kills
  its own devnet. The binary under test is the real `lessdugin` with `LESSDUGIN_KEYRING=off`
  (headless Linux has no Secret Service, so the 0600 file fallback is exercised).
- No public infrastructure was contacted.

## Results

| Step | Observed on the devnet |
|---|---|
| Protected-resource metadata of the PDS | one authorization server, the PDS itself |
| PAR with the loopback `client_id` `http://localhost?redirect_uri=<pct>&scope=<pct(SUPERSET)>` (no port, empty path) | accepted (201) after one `use_dpop_nonce` round trip |
| Scope (`.temp` build): `atproto repo?collection=NS.forum&…(10) blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf` | accepted at PAR and granted unchanged in the token response |
| Sign-in and consent pages | identifier prefilled from `login_hint`; **Authorize** redirects with `?state&iss&code` (`response_mode=query`) |
| `--no-browser` paste-back | the pasted URL is checked (target, `state`, `iss`); code exchanged with the PKCE verifier |
| Loopback listener on `127.0.0.1:<random port>` | callback received; foreign requests (other paths, wrong `state`) are answered and ignored |
| Token response | `token_type: DPoP`, `sub` = the account DID |
| Login binding (core) | fresh DID document → PDS → its protected-resource metadata lists `iss`; issuer pinned, shown as "first login: pinned" |
| `whoami` (`getSession`, DPoP to the PDS) | 401 `use_dpop_nonce`, then 200 |
| `putRecord` of `online.7778777.lessdugin.temp.post` through the stored OAuth session | 200; `lessdugin verify` of the record: `proven` |
| Refresh (stored expiry forced into the past) | new DPoP-bound tokens, refresh token rotated, saved under the refresh lock |
| `logout` | revocation at the revocation endpoint accepted; session file deleted; `whoami` → exit 3 |
| App-password path, same test | `createSession` → `whoami` → `resolve` → raw-XRPC record → `verify` `proven` / `absent`; proactive and reactive (`400 InvalidToken`) refresh; `deleteSession` on logout |

The whole scenario (three logins, two writes, two verifications, three refreshes) runs in about
14 s including the devnet boot.

### What jacquard-oauth provides, and where the facade departs from it

Used as is: `request::oauth_request` (PAR, token, refresh and revocation requests with DPoP and
the authorization-server nonce retry), `dpop::build_dpop_proof`, `utils::{generate_dpop_key,
generate_pkce, generate_nonce}`, `resolver::{resolve_protected_resource_info,
resolve_authorization_server}`, and the `types` and `session::DpopReqData`. All of its HTTP goes
through `Bridge`, a `jacquard_common::HttpClient` over `Net::send_http` (SSRF guard, 64 KiB cap,
User-Agent, no redirects).

Not used, with the reason (each one a gap of 0.12.2 against our requirements):

1. `request::par` sends no `response_mode`; `data-model.md` §10.5 requires `query`. The facade
   builds `ParParameters` itself and calls `oauth_request`.
2. `AtprotoClientMetadata::new_localhost` produces `http://localhost/?…` with a normalised
   scope; §10.4 fixes `http://localhost?…` with `SUPERSET`. The facade builds the client id.
3. `request::exchange_code` and `OAuthResolver::verify_issuer` resolve the token's `sub` with
   jacquard-identity's resolver (its own DNS, HTTP client and handle rules, outside the SSRF
   guard and the devnet rules). The facade sends the token request with `oauth_request` and
   leaves the issuer check to the core's login binding, which does the same check with the
   core's resolver. `Bridge`'s `IdentityResolver` implementation refuses every call, so an
   accidental use fails loudly.
4. `dpop::wrap_request_with_dpop` puts the full request URI, query included, in `htu`;
   RFC 9449 §4.2 excludes query and fragment. PDS requests are signed by the facade with
   `build_dpop_proof`, `htu` without the query, `ath`, and the PDS nonce (one retry on
   `use_dpop_nonce`).
5. `OAuthClient`/`OAuthSession` (the high-level API) own their store, cache and refresh logic
   with no cross-process lock. The facade keeps its own session (`OAuthSession` in
   `lessdugin_native::auth::oauth`), refreshing under `<data>/locks/refresh-<h(did)>.lock` and
   re-reading the stored session first.
6. `revoke` expects HTTP 204 only; the facade also accepts 200 (RFC 7009).

## Decision

1. Native OAuth is `lessdugin_native::auth::oauth` over `jacquard-oauth` `=0.12.2`'s low-level
   pieces as listed above; the 1.5k-line fallback client is not needed.
2. A login is: core identity resolution → PAR (scope candidates; hosted metadata after
   `invalid_client`) → browser or paste-back → `state`/`iss` check → token request with the PKCE
   verifier → `Engine::bind_login` + `add_account` → only then the session is stored and used.
3. OAuth and app-password sessions are stored as one JSON secret in the OS keyring (service
   `lessdugin`, account = DID) or, without a keyring, in `<data>/sessions/<h(did)>.json` (0600,
   `O_EXCL | O_NOFOLLOW`, atomic rename; refused if a symlink or group/world accessible).
   `LESSDUGIN_KEYRING=off` forces the file.
4. Every native `AuthSession` applies the AUTH-4 allowlist before authenticating a request.

## Consequences

- S1 passes against the devnet. **Pending user run** (production, needs real accounts):
  - a `bsky.social` account, where the PDS (`*.host.bsky.network`) lists the entryway issuer
    `https://bsky.social`: login binding and pin across hosts;
  - `--oauth-client hosted` and the automatic fallback after `invalid_client`, against the
    deployed `deploy/site/oauth/cli-client-metadata.json` (the devnet cannot fetch it). That file
    lists final-namespace scopes only, so a `.temp` build using the hosted client is expected to
    get `invalid_scope`; release builds (`final-ns`) are unaffected;
  - `include:online.7778777.lessdugin.authFull` with a `final-ns` build after the permission set
    is published (S8 records how an unresolvable `include:` fails at PAR);
  - the OS keyring backends on macOS, Windows and a Linux desktop (only the file fallback ran
    here).
- The PDS's DPoP nonce is kept in memory and saved only with token refreshes, so the first PDS
  request of each process costs one extra round trip.
- If a later jacquard release fixes items 1–4, the facade can shrink; the facade's API does not
  depend on them.
