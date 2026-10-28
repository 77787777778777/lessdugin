// SPDX-License-Identifier: AGPL-3.0-or-later
// An authority unbans by removing their own #access record, never by granting membership
// (which would make the user a seed, social.md §4.6), and can reject an applicant (review
// findings m3-9, m3-11). Every action but the local reject passes the moderation interstitial
// first (social.md §17; m3-4, sec-4).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView, MemberView } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../../test/render.tsx";
import { ForumMembersPage } from "./ForumMembersPage.tsx";
import { ForumContext } from "./forumContext.tsx";

const member = (did: string, over: Partial<MemberView>): MemberView => ({
  actor: { did, shortDid: did.slice(0, 12), handleState: "unverified" },
  status: "member",
  granted: false,
  joinedAtMs: 0,
  ...over,
});

async function setup(members: MemberView[]) {
  const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
  const forum = (await engine.followed())[0]?.uri ?? "";
  const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
  delete view.viewer;
  vi.spyOn(engine, "members").mockResolvedValue(members);
  renderWithApp(
    <ForumContext value={view}>
      <ForumMembersPage />
    </ForumContext>,
    { engine },
  );
  return { engine, forum };
}

const ACK = "Moderation actions are public";

async function acknowledge(): Promise<void> {
  const ack = await screen.findByRole("dialog", { name: ACK });
  fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
}

describe("ForumMembersPage actions", () => {
  it("offers Unban, not Grant, for a banned member, and removes the own record", async () => {
    const banned = "did:plc:bannedbannedbannedbanned";
    const { engine, forum } = await setup([member(banned, { banned: true })]);
    const grant = vi.spyOn(engine, "grant");
    const revoke = vi.spyOn(engine, "revoke");
    const row = (await screen.findByText("Banned")).closest("tr") as HTMLElement;
    expect(within(row).queryByRole("button", { name: "Grant" })).toBeNull();
    fireEvent.click(within(row).getByRole("button", { name: "Unban" }));
    await acknowledge();
    await waitFor(() => expect(revoke).toHaveBeenCalledWith(forum, [banned]));
    expect(grant).not.toHaveBeenCalled();
  });

  it("approves or rejects an applicant", async () => {
    const applicant = "did:plc:applicantapplicantapplic";
    const { engine, forum } = await setup([member(applicant, { applicant: true })]);
    const reject = vi.spyOn(engine, "reject");
    const row = (await screen.findByText("Applicant")).closest("tr") as HTMLElement;
    expect(within(row).getByRole("button", { name: "Approve" })).toBeTruthy();
    fireEvent.click(within(row).getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(reject).toHaveBeenCalledWith(forum, [applicant]));
    // rejecting is local: no interstitial
    expect(screen.queryByRole("dialog", { name: ACK })).toBeNull();
  });

  it("asks before the first grant or approve, and sends nothing when cancelled", async () => {
    const plain = "did:plc:plainplainplainplainplai";
    const applicant = "did:plc:applicantapplicantapplic";
    const { engine, forum } = await setup([
      member(plain, {}),
      member(applicant, { applicant: true }),
    ]);
    const grant = vi.spyOn(engine, "grant");
    const approve = vi.spyOn(engine, "approve");
    const plainRow = (await screen.findByText(plain.slice(0, 12))).closest("tr") as HTMLElement;
    fireEvent.click(within(plainRow).getByRole("button", { name: "Grant" }));
    const ack = await screen.findByRole("dialog", { name: ACK });
    expect(grant).not.toHaveBeenCalled();
    fireEvent.click(within(ack).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: ACK })).toBeNull());
    expect(grant).not.toHaveBeenCalled();
    const applicantRow = (await screen.findByText("Applicant")).closest("tr") as HTMLElement;
    fireEvent.click(within(applicantRow).getByRole("button", { name: "Approve" }));
    expect(approve).not.toHaveBeenCalled();
    await acknowledge();
    await waitFor(() => expect(approve).toHaveBeenCalledWith(forum, [applicant]));
    // acknowledged once: the next action is sent at once
    fireEvent.click(within(plainRow).getByRole("button", { name: "Grant" }));
    await waitFor(() => expect(grant).toHaveBeenCalledWith(forum, [plain]));
    expect(screen.queryByRole("dialog", { name: ACK })).toBeNull();
  });

  it("lifts the own restriction through a removal", async () => {
    const restricted = "did:plc:restrictedrestrictedrest";
    const { engine, forum } = await setup([
      member(restricted, { granted: true, restricted: ["comment"] }),
    ]);
    const mod = vi.spyOn(engine, "modAction");
    const row = (await screen.findByText("Restricted")).closest("tr") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: "Lift restriction" }));
    await acknowledge();
    await waitFor(() =>
      expect(mod).toHaveBeenCalledWith({
        kind: "remove",
        forum,
        scope: "forum",
        subject: restricted,
        actionKey: "restrict",
      }),
    );
  });
});
