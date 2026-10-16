// SPDX-License-Identifier: AGPL-3.0-or-later
// #/at/* resolves an at-uri (or did) with the engine's parser, then shows the page for it. A
// record that is not one of our pages gets the raw view: the record JSON, its validity and every
// moderation assertion on it (social.md §4.12).

import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { useEffect } from "react";
import { useOpenRef } from "../components/RecordLink.tsx";
import { ErrorView, Loading } from "../components/Status.tsx";
import { useEngine } from "../engine/context.tsx";
import { qk, useParsedRef } from "../engine/queries.ts";
import ui from "../styles/ui.module.css";
import styles from "./pages.module.css";

const route = getRouteApi("/at/$");

function RawRecord({ uri }: { uri: string }) {
  const engine = useEngine();
  const raw = useQuery({ queryKey: qk.raw(uri), queryFn: () => engine.raw(uri) });
  const verify = useQuery({
    queryKey: ["verify", uri],
    queryFn: () => engine.verify(uri, false),
    enabled: false,
  });
  return (
    <div className={ui.page}>
      <h1 className={ui.pageTitle}>Record</h1>
      <p className={ui.mono}>{uri}</p>
      {raw.isPending && <Loading />}
      {raw.isError && <ErrorView error={raw.error} title="This record is not available" />}
      {raw.data && (
        <>
          <dl className={styles.kv}>
            <dt>CID</dt>
            <dd className={ui.mono}>{raw.data.cid ?? "unknown"}</dd>
            <dt>Trust level</dt>
            <dd>{raw.data.level}</dd>
            <dt>Validity</dt>
            <dd>{raw.data.validity.length === 0 ? "valid" : raw.data.validity.join(", ")}</dd>
            {raw.data.state && (
              <>
                <dt>State</dt>
                <dd>{raw.data.state.kind}</dd>
              </>
            )}
          </dl>
          <h2 className={ui.sectionTitle}>Record</h2>
          <pre className={styles.raw}>{JSON.stringify(raw.data.record ?? null, null, 2)}</pre>
          <h2 className={ui.sectionTitle}>Assertions</h2>
          <pre className={styles.raw}>{JSON.stringify(raw.data.assertions, null, 2)}</pre>
        </>
      )}
      <p>
        <button type="button" className={ui.button} onClick={() => void verify.refetch()}>
          Verify with a commit proof
        </button>
      </p>
      {verify.data && (
        <ul className={ui.list}>
          <li>
            Result: <strong>{verify.data.result}</strong> ({verify.data.keySource})
          </li>
          {verify.data.checks.map((c) => (
            <li key={c.name}>
              {c.ok ? "✓" : "✗"} {c.name}
              {c.detail && <span className={ui.muted}> — {c.detail}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function AtPage() {
  const { _splat } = route.useParams();
  const input =
    _splat?.startsWith("did:") && !_splat.includes("/") ? _splat : `at://${_splat ?? ""}`;
  const ref = useParsedRef(input);
  const openRef = useOpenRef();
  const target = ref.data;
  useEffect(() => {
    if (target && target.kind !== "record") openRef(target, { replace: true });
  }, [target, openRef]);
  if (ref.isPending) return <Loading label="Resolving…" />;
  if (ref.isError)
    return <ErrorView page error={ref.error} title="This link could not be resolved" />;
  if (ref.data.kind === "record") return <RawRecord uri={ref.data.uri} />;
  return <Loading label="Opening…" />;
}
