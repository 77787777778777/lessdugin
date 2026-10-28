// SPDX-License-Identifier: AGPL-3.0-or-later
import type { MemberView } from "@lessdugin/engine/types";
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

/** What an authority can do about a member: approve or reject an applicant, unban (the
 * removal of one's own `#access` record: a ban is not lifted by granting, which would make the
 * user a seed, social.md §4.6), lift one's own restriction, or grant and revoke membership. */
type MemberAction = "grant" | "revoke" | "unban" | "approve" | "reject" | "unrestrict";

const DONE: Record<MemberAction, string> = {
  grant: "Membership granted",
  revoke: "Grant revoked",
  unban: "Your ban was removed",
  approve: "Applicant approved",
  reject: "Applicant rejected",
  unrestrict: "Your restriction was removed",
};

const LABEL: Record<MemberAction, string> = {
  grant: "Grant",
  revoke: "Revoke",
  unban: "Unban",
  approve: "Approve",
  reject: "Reject",
  unrestrict: "Lift restriction",
};
const TITLE: Partial<Record<MemberAction, string>> = {
  unban: "Removes your own ban record; a ban by a higher-ranked authority stays",
  unrestrict: "Removes your own restriction record",
  reject: "Dismisses the application on this device",
};

export function ForumMembersPage() {
  const engine = useEngine();
  const view = useForumView();
  const uri = view.forum.uri;
  const authority = view.viewer !== undefined ? view.viewer.moderate : isAuthorityRole(view.myRole);
  const q = useQuery({ queryKey: qk.members(uri), queryFn: () => engine.members(uri, {}) });
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorView error={q.error} />;
  const act = async (did: string, action: MemberAction): Promise<void> => {
    try {
      const tickets =
        action === "approve"
          ? await engine.approve(uri, [did])
          : action === "reject"
            ? await engine.reject(uri, [did])
            : action === "unrestrict"
              ? [
                  await engine.modAction({
                    kind: "remove",
                    forum: uri,
                    scope: "forum",
                    subject: did,
                    actionKey: "restrict",
                  }),
                ]
              : [
                  await (action === "grant"
                    ? engine.grant(uri, [did])
                    : // `revoke` deletes the account's own `#access` record, a ban included
                      engine.revoke(uri, [did])),
                ];
      let undone = false;
      for (const t of tickets) {
        writeStore.track(t);
        const final = await writeStore.wait(t.id);
        if (final.kind === "failed") throw final.error;
        if (final.kind !== "done") undone = true;
      }
      toasts.show(undone ? { message: "Undone" } : { message: DONE[action], tone: "success" });
    } catch (e) {
      toasts.show({ message: errorMessage(e), tone: "error" });
    }
  };
  const actionsFor = (m: MemberView): MemberAction[] => {
    const out: MemberAction[] = [];
    if (m.banned === true) out.push("unban");
    else if (m.applicant === true) out.push("approve", "reject");
    else out.push(m.granted ? "revoke" : "grant");
    if ((m.restricted?.length ?? 0) > 0 || m.nullified === true) out.push("unrestrict");
    return out;
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
              {authority && <th scope="col">Actions</th>}
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
                  <td className={ui.row}>
                    {actionsFor(m).map((a) => (
                      <button
                        key={a}
                        type="button"
                        className={ui.ghost}
                        title={TITLE[a]}
                        onClick={() => void act(m.actor.did, a)}
                      >
                        {LABEL[a]}
                      </button>
                    ))}
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
