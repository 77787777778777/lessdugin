// SPDX-License-Identifier: AGPL-3.0-or-later
// Local atproto network for end-to-end tests: PLC + PDS, no AppView (lessdugin needs none).
// Usage: node devnet.mjs [--accounts alice,bob,carol] [--json state.json]
// Prints (or writes) a JSON description of the network and keeps running until SIGINT/SIGTERM.

import { writeFileSync } from "node:fs";
import { TestNetworkNoAppView } from "@atproto/dev-env";

const args = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const names = arg("--accounts", "alice,bob,carol").split(",").filter(Boolean);
const jsonOut = arg("--json", null);

const started = Date.now();
const network = await TestNetworkNoAppView.create({ dbPostgresSchema: "lessdugin_e2e" });
const accounts = {};
// Plain XRPC over fetch keeps this script independent of SDK API churn.
const xrpc = async (nsid, body, token) => {
  const res = await fetch(`${network.pds.url}/xrpc/${nsid}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });
  const json = await res.json();
  if (!res.ok) throw new Error(`${nsid}: ${res.status} ${JSON.stringify(json)}`);
  return json;
};
for (const name of names) {
  const handle = `${name}.test`;
  const password = `${name}-password`;
  const acct = await xrpc("com.atproto.server.createAccount", {
    handle,
    email: `${name}@example.test`,
    password,
  });
  const appPw = await xrpc("com.atproto.server.createAppPassword", { name: "e2e" }, acct.accessJwt);
  accounts[name] = { handle, did: acct.did, password, appPassword: appPw.password };
}
const state = {
  pds: network.pds.url,
  plc: network.plc.url,
  accounts,
  bootMs: Date.now() - started,
};
const text = JSON.stringify(state, null, 2);
if (jsonOut) writeFileSync(jsonOut, text);
console.log(text);

const shutdown = async () => {
  await network.close();
  process.exit(0);
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
