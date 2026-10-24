// SPDX-License-Identifier: AGPL-3.0-or-later
import { useQuery } from "@tanstack/react-query";
import { ActorName } from "../../components/ActorName.tsx";
import { ErrorView, errorMessage, Loading } from "../../components/Status.tsx";
import { toasts } from "../../components/Toast.tsx";
import { useEngine } from "../../engine/context.tsx";
import { qk } from "../../engine/queries.ts";
import { writeStore } from "../../engine/writes.ts";
import { fullDate } from "../../lib/time.ts";
import ui from "../../styles/ui.module.css";
import { isAuthorityRole, useForumView } from "./forumContext.tsx";

export function ForumMembersPage() {
  const engine = useEngine();
  const view = useForumView();
  const uri = view.forum.uri;
  const authority = view.viewer !== undefined ? view.viewer.moderate : isAuthorityRole(view.myRole);
  const q = useQuery({ queryKey: qk.members(uri), queryFn: () => engine.members(uri, {}) });
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView error={q.error} />;
  const grant = async (did: string, on: boolean): Promise<void> => {
    try {
      const t = await (on ? engine.grant(uri, [did]) : engine.revoke(uri, [did]));
      writeStore.track(t);
      const final = await writeStore.wait(t.id);
      if (final.kind === "failed") throw final.error;
      if (final.kind === "done") {
        toasts.show({ message: on ? "Membership granted" : "Grant revoked", tone: "success" });
      } else {
        toasts.show({ message: on ? "Grant undone" : "Revocation undone" });
      }
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };
  return (
    <section className={ui.pageWide} aria-label="Members">
      <h2 className={ui.sectionTitle}>Members ({q.data.length})</h2>
      <div className={ui.tableScroll}>
        <table className={ui.table}>
          <thead>
            <tr>
              <th scope="col">Member</th>
              <th scope="col">Status</th>
              <th scope="col">Joined</th>
              <th scope="col">Note</th>
              <th scope="col">Flags</th>
              {authority && <th scope="col">Grant</th>}
            </tr>
          </thead>
          <tbody>
            {q.data.map((m) => (
              <tr key={m.actor.did}>
                <td>
                  <ActorName actor={m.actor} showIdent />
                </td>
                <td>
                  {m.status}
                  {m.granted && <span className={ui.muted}> (granted)</span>}
                  {m.karma !== undefined && <span className={ui.muted}> · karma {m.karma}</span>}
                </td>
                <td>{fullDate(m.joinedAtMs)}</td>
                <td className={ui.muted}>{m.membershipNote}</td>
                <td data-testid="member-flags">
                  {m.applicant === true && (
                    <span className={ui.chip} data-flag="applicant">
                      Applicant
                    </span>
                  )}
                  {m.banned === true && (
                    <span className={ui.chip} data-flag="banned">
                      Banned
                    </span>
                  )}
                  {m.nullified === true && (
                    <span
                      className={ui.chip}
                      data-flag="nullified"
                      title="Their votes carry no weight"
                    >
                      Nullified
                    </span>
                  )}
                  {m.restricted !== undefined && m.restricted.length > 0 && (
                    <span
                      className={ui.chip}
                      data-flag="restricted"
                      title={`Restricted from: ${m.restricted.join(", ")}`}
                    >
                      Restricted
                    </span>
                  )}
                </td>
                {authority && (
                  <td>
                    <button
                      type="button"
                      className={ui.ghost}
                      onClick={() => void grant(m.actor.did, !m.granted)}
                    >
                      {m.granted ? "Revoke" : "Grant"}
                    </button>
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
