// SPDX-License-Identifier: AGPL-3.0-or-later
// Local settings (synthesis §6.12): accounts, sources, verification, indexer consent, external
// images, media autoload hosts, karma notifications, export/import of local state, storage
// persistence, and the running build's hash (threat-model SUP-4).

import type {
  AccountView,
  AuditReport,
  IndexerConsent,
  Json,
  Settings,
} from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import { useAccountActor } from "../components/AccountName.tsx";
import { ActorName } from "../components/ActorName.tsx";
import { ErrorView, errorMessage, Loading } from "../components/Status.tsx";
import { toasts } from "../components/Toast.tsx";
import { useEngine, useEngineHandle } from "../engine/context.tsx";
import { useCurrentForum } from "../engine/currentForum.ts";
import { qk, useFollowed, useSettings } from "../engine/queries.ts";
import { downloadBlob } from "../lib/download.ts";
import { cleanName } from "../lib/text.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

const HOST = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;

/** threat-model §5.x: forum-declared endpoints need https, a hostname and the default port. */
export function acceptableIndexerUrl(raw: string): boolean {
  try {
    const u = new URL(raw);
    return u.protocol === "https:" && u.port === "" && u.username === "" && HOST.test(u.hostname);
  } catch {
    return false;
  }
}

/** Changes one setting; a refusal is shown, never left as an unhandled rejection. */
export function useSetSetting() {
  const engine = useEngine();
  const qc = useQueryClient();
  return async <K extends keyof Settings>(key: K, value: Settings[K]): Promise<boolean> => {
    try {
      // the cadence has its own operation (engine-api.md §3.6)
      if (key === "karmaNotifications") {
        await engine.setKarmaNotifications(value as Settings["karmaNotifications"]);
      } else {
        await engine.setSetting(key, value);
      }
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
      return false;
    }
    await qc.invalidateQueries({ queryKey: qk.settings() });
    if (key === "indexers") await qc.invalidateQueries({ queryKey: ["forumIndexers"] });
    return true;
  };
}

function AccountRow({ account }: { account: AccountView }) {
  const actor = useAccountActor(account);
  return actor ? <ActorName actor={actor} showIdent /> : null;
}

function Accounts() {
  const engine = useEngine();
  const qc = useQueryClient();
  const accounts = useQuery({ queryKey: qk.accounts(), queryFn: () => engine.accounts() });
  return (
    <section aria-labelledby="settings-accounts">
      <h2 id="settings-accounts" className={ui.sectionTitle}>
        Accounts
      </h2>
      {accounts.data?.length === 0 && <p className={ui.muted}>No account signed in.</p>}
      <ul className={ui.list}>
        {accounts.data?.map((a) => (
          <li key={a.did} className={ui.row}>
            <AccountRow account={a} />
            <span className={ui.muted}>{a.pds}</span>
            {a.active ? (
              <span className={ui.chipActive}>active</span>
            ) : (
              <button
                type="button"
                className={ui.ghost}
                onClick={() =>
                  void engine.setActiveAccount(a.did).then(() => qc.invalidateQueries())
                }
              >
                Switch to
              </button>
            )}
            <button
              type="button"
              className={ui.ghost}
              onClick={() =>
                void engine.removeAccount(a.did, false).then(() => qc.invalidateQueries())
              }
            >
              Sign out
            </button>
          </li>
        ))}
      </ul>
      <Link to="/login">Add an account</Link>
    </section>
  );
}

function HostList({ hosts, onChange }: { hosts: string[]; onChange: (hosts: string[]) => void }) {
  const [value, setValue] = useState("");
  const id = useId();
  const host = value.trim().toLowerCase();
  const valid = HOST.test(host);
  return (
    <div>
      <ul className={ui.list}>
        {hosts.map((h) => (
          <li key={h} className={ui.row}>
            <span className={ui.mono}>{h}</span>
            <button
              type="button"
              className={ui.ghost}
              onClick={() => onChange(hosts.filter((x) => x !== h))}
            >
              Remove
            </button>
          </li>
        ))}
      </ul>
      <div className={ui.row}>
        <label htmlFor={id} className="visually-hidden">
          Host
        </label>
        <input
          id={id}
          className={ui.input}
          placeholder="pds.example.com"
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
        <button
          type="button"
          className={ui.button}
          disabled={!valid || hosts.includes(host)}
          onClick={() => {
            onChange([...hosts, host]);
            setValue("");
          }}
        >
          Add host
        </button>
      </div>
    </div>
  );
}

function BuildInfo() {
  const handle = useEngineHandle();
  const build = useQuery({
    queryKey: ["build-info"],
    queryFn: async () => {
      const r = await fetch("./build-info.json", { cache: "no-store" });
      if (!r.ok) throw new Error("no build-info.json (development build)");
      return (await r.json()) as { sha256: string };
    },
    retry: false,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const version = useQuery({
    queryKey: ["engine-version", handle.kind],
    queryFn: () => handle.version(),
    retry: false,
  });
  return (
    <section aria-labelledby="settings-about">
      <h2 id="settings-about" className={ui.sectionTitle}>
        About this build
      </h2>
      <dl className={styles.kv}>
        <dt>Build hash (SHA-256)</dt>
        <dd className={ui.mono} data-testid="build-hash">
          {build.data?.sha256 ?? (build.isError ? "development build (no published hash)" : "…")}
        </dd>
        <dt>Engine</dt>
        <dd>
          {handle.kind === "mock" ? "mock" : "wasm"}
          {version.data && <span className={ui.muted}> · {version.data}</span>}
          {version.isError && <span className={ui.muted}> · {errorMessage(version.error)}</span>}
        </dd>
        <dt>Licence</dt>
        <dd>GNU AGPL v3 or later</dd>
      </dl>
      <p className={ui.small}>
        Compare the build hash with the one published for the release. For a zero-trust setup, run{" "}
        <code>lessdugin web</code> and use the app it serves from your own machine.
      </p>
    </section>
  );
}

function LocalData() {
  const engine = useEngine();
  const qc = useQueryClient();
  const fileId = useId();
  const [persisted, setPersisted] = useState<boolean | null>(null);
  const exportLocal = async (): Promise<void> => {
    const json = await engine.exportLocal();
    const blob = new Blob([`${JSON.stringify(json, null, 2)}\n`], { type: "application/json" });
    downloadBlob(blob, "lessdugin-local-state.json");
  };
  const importLocal = async (file: File): Promise<void> => {
    try {
      const json = JSON.parse(await file.text()) as Json;
      await engine.importLocal(json);
      await qc.invalidateQueries();
      toasts.show({ message: "Local state imported", tone: "success" });
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };
  return (
    <section aria-labelledby="settings-local">
      <h2 id="settings-local" className={ui.sectionTitle}>
        Local data
      </h2>
      <p className={ui.small}>
        Drafts, read marks, bookmarks, subscriptions, mutes and these settings stay on this device.
      </p>
      <div className={ui.row}>
        <button type="button" className={ui.button} onClick={() => void exportLocal()}>
          Export local state
        </button>
        <label htmlFor={fileId} className={ui.button}>
          Import…
          <input
            id={fileId}
            type="file"
            accept="application/json"
            className="visually-hidden"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void importLocal(f);
              e.target.value = "";
            }}
          />
        </label>
        <button
          type="button"
          className={ui.ghost}
          onClick={() =>
            void (navigator.storage?.persist?.() ?? Promise.resolve(false)).then(setPersisted, () =>
              setPersisted(false),
            )
          }
        >
          Keep storage persistent
        </button>
        {persisted !== null && (
          <span className={ui.muted}>
            {persisted ? "Storage is persistent." : "The browser declined."}
          </span>
        )}
      </div>
    </section>
  );
}

function sameIndexer(a: string, b: string): boolean {
  try {
    return new URL(a).href.replace(/\/$/, "") === new URL(b).href.replace(/\/$/, "");
  } catch {
    return a === b;
  }
}

function ForumDeclaredIndexers({ forum }: { forum: { uri: string; name: string } }) {
  const engine = useEngine();
  const settings = useSettings();
  const set = useSetSetting();
  const declared = useQuery({
    queryKey: qk.forumIndexers(forum.uri),
    queryFn: () => engine.forumIndexers(forum.uri),
  });
  if (declared.isError) return <ErrorView error={declared.error} />;
  if (!declared.data?.length || !settings.data) return null;
  const consents = settings.data.indexers;
  return (
    <div>
      <h3 className={ui.sectionTitle}>{cleanName(forum.name)}</h3>
      <ul className={ui.list}>
        {declared.data.map((i) => (
          <li key={i.url} className={ui.row}>
            <span>
              <span className={ui.mono}>{i.url}</span>
              {i.note && <span className={ui.hint}> {i.note}</span>}
            </span>
            <button
              type="button"
              className={ui.ghost}
              onClick={() => {
                const without = consents.filter(
                  (x) => !(x.forum === forum.uri && sameIndexer(x.url, i.url)),
                );
                const next: IndexerConsent = {
                  url: i.url,
                  forum: forum.uri,
                  grantedAtMs: Date.now(),
                };
                if (i.did !== undefined) next.did = i.did;
                void set("indexers", i.consented ? without : [...without, next]);
              }}
            >
              {i.consented ? "Revoke consent" : "Consent for this forum"}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function IndexerAudit({ forums }: { forums: { uri: string; name: string }[] }) {
  const engine = useEngine();
  const [forum, setForum] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const selected = forum || forums[0]?.uri || "";
  const run = async (): Promise<void> => {
    if (!selected) return;
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      setReport(await engine.auditIndexer(selected));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div>
      <h3 className={ui.sectionTitle}>Manual indexer audit (A1–A5)</h3>
      <p className={ui.small}>
        Cross-check exactly one consented indexer against PDS heads and local recomputation.
        Findings are reported only; this never demotes an indexer automatically.
      </p>
      <div className={ui.row}>
        <select
          className={ui.input}
          aria-label="Forum to audit"
          value={selected}
          onChange={(e) => setForum(e.target.value)}
        >
          {forums.map((f) => (
            <option key={f.uri} value={f.uri}>
              {cleanName(f.name)}
            </option>
          ))}
        </select>
        <button
          type="button"
          className={ui.button}
          disabled={!selected || busy}
          onClick={() => void run()}
        >
          {busy ? "Auditing…" : "Run audit"}
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
      {report && (
        <div aria-live="polite">
          <p>
            Checked {report.counts.checked}; {report.counts.contradictions} contradictions;{" "}
            {report.counts.omissions} omissions; {report.counts.lag} lag findings.
          </p>
          <ul className={ui.list}>
            {report.checks.flatMap((c) =>
              c.findings.map((f) => (
                <li key={`${c.name}:${f.check}:${f.subject}:${f.kind}:${f.detail}`}>
                  <strong>
                    {f.check} {f.kind}
                  </strong>
                  : {f.detail} <span className={ui.mono}>{f.subject}</span>
                </li>
              )),
            )}
          </ul>
        </div>
      )}
    </div>
  );
}

export function SettingsPage() {
  const settings = useSettings();
  const followed = useFollowed();
  const currentForum = useCurrentForum();
  const set = useSetSetting();
  const [indexerUrl, setIndexerUrl] = useState("");
  if (settings.isPending) return <Loading />;
  if (settings.isError) return <ErrorView page error={settings.error} />;
  const s = settings.data;
  const radio = <K extends "sources" | "verify" | "karmaNotifications">(
    key: K,
    options: { value: Settings[K]; label: string; hint: string }[],
  ) =>
    options.map((o) => (
      <label key={String(o.value)} className={ui.row}>
        <input
          type="radio"
          name={key}
          checked={s[key] === o.value}
          onChange={() => void set(key, o.value)}
        />
        <span>
          {o.label} <span className={ui.hint}>{o.hint}</span>
        </span>
      </label>
    ));
  const forumChoices = [...(followed.data ?? [])];
  if (currentForum && !forumChoices.some((f) => f.uri === currentForum)) {
    forumChoices.push({ uri: currentForum, name: currentForum } as (typeof forumChoices)[number]);
  }
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Settings</h1>
      <Accounts />
      <section aria-labelledby="settings-sources">
        <h2 id="settings-sources" className={ui.sectionTitle}>
          Data sources
        </h2>
        {radio("sources", [
          {
            value: "assisted",
            label: "Assisted (default)",
            hint: "Also use relays, Jetstream and Constellation as hints to find content faster.",
          },
          {
            value: "direct",
            label: "Direct",
            hint: "Only PDSes, PLC and DNS. Slower; some newcomers stay unseen.",
          },
        ])}
      </section>
      <section aria-labelledby="settings-verify">
        <h2 id="settings-verify" className={ui.sectionTitle}>
          Verification
        </h2>
        {radio("verify", [
          {
            value: "pds",
            label: "PDS (default)",
            hint: "Trust content fetched from the author's PDS over TLS.",
          },
          {
            value: "proof",
            label: "Proof",
            hint: "Also check signed commit proofs for every item shown, in the background.",
          },
        ])}
      </section>
      <section aria-labelledby="settings-indexers">
        <h2 id="settings-indexers" className={ui.sectionTitle}>
          Indexers
        </h2>
        <p className={ui.small}>
          Optional servers that speed up large forums. Their answers are hints; scores and
          moderation are always recomputed here. A forum-declared indexer is used only after you
          consent.
        </p>
        <ul className={ui.list}>
          {s.indexers.map((i) => (
            <li key={`${i.forum ?? "global"}:${i.url}`} className={ui.row}>
              <span className={ui.mono}>{i.url}</span>
              <button
                type="button"
                className={ui.ghost}
                onClick={() =>
                  void set(
                    "indexers",
                    s.indexers.filter((x) => !(x.forum === i.forum && sameIndexer(x.url, i.url))),
                  )
                }
              >
                Revoke consent
              </button>
            </li>
          ))}
        </ul>
        <div className={ui.row}>
          <input
            className={ui.input}
            placeholder="https://indexer.example.com"
            aria-label="Indexer URL"
            value={indexerUrl}
            onChange={(e) => setIndexerUrl(e.target.value)}
          />
          <button
            type="button"
            className={ui.button}
            disabled={!acceptableIndexerUrl(indexerUrl.trim())}
            onClick={async () => {
              const saved = await set("indexers", [
                ...s.indexers.filter((x) => x.forum !== undefined),
                { url: indexerUrl.trim(), grantedAtMs: Date.now() },
              ]);
              if (saved) setIndexerUrl("");
            }}
          >
            Consent and add
          </button>
        </div>
        {forumChoices.map((f) => (
          <ForumDeclaredIndexers key={f.uri} forum={f} />
        ))}
        <IndexerAudit forums={forumChoices} />
      </section>
      <section aria-labelledby="settings-images">
        <h2 id="settings-images" className={ui.sectionTitle}>
          External images
        </h2>
        <p className={ui.small}>
          Images hosted elsewhere are never fetched by default: loading one tells that site your IP
          address.
        </p>
        {followed.data?.map((f) => (
          <label key={f.uri} className={ui.row}>
            <input
              type="checkbox"
              checked={s.externalImagesForums.includes(f.uri)}
              onChange={(e) =>
                void set(
                  "externalImagesForums",
                  e.target.checked
                    ? [...s.externalImagesForums, f.uri]
                    : s.externalImagesForums.filter((x) => x !== f.uri),
                )
              }
            />
            <span>
              Load external images in <bdi>{cleanName(f.name)}</bdi>
            </span>
          </label>
        ))}
      </section>
      <section aria-labelledby="settings-media">
        <h2 id="settings-media" className={ui.sectionTitle}>
          Media autoload
        </h2>
        <p className={ui.small}>
          Media from these hosts loads without a click (every file is still checked against its
          CID).
        </p>
        <HostList
          hosts={s.mediaAutoloadHosts}
          onChange={(h) => void set("mediaAutoloadHosts", h)}
        />
      </section>
      <section aria-labelledby="settings-karma">
        <h2 id="settings-karma" className={ui.sectionTitle}>
          Karma notifications
        </h2>
        {radio("karmaNotifications", [
          { value: "realtime", label: "Real time", hint: "" },
          { value: "daily", label: "Daily (default)", hint: "" },
          { value: "weekly", label: "Weekly", hint: "" },
        ])}
      </section>
      <LocalData />
      <BuildInfo />
    </div>
  );
}
