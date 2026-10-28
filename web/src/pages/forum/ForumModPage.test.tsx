// SPDX-License-Identifier: AGPL-3.0-or-later
// The moderation page (review findings m3r-13, m3-11, sec-5): the log pages through the
// engine's cursor, a `blocked` queue answer is recognised by its code, queue items take an
// action, the viewer's own assertions can be removed from the log, and a former moderator's
// assertions can be adopted.
import { MockEngine } from "@lessdugin/engine/mock";
import { DTO_VERSION, type ForumView, type ModLogEntry } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../../test/render.tsx";
import { ForumModPage, isBlocked } from "./ForumModPage.tsx";
import { ForumContext } from "./forumContext.tsx";

const OTHER = "did:plc:othermodothermodothermod";

function entry(over: Partial<ModLogEntry>): ModLogEntry {
  return {
    uri: "at://did:plc:othermodothermodothermod/online.7778777.lessdugin.temp.modAction/x-1",
    actor: { did: OTHER, shortDid: "did:plc:othe…", handleState: "unverified" },
    action: "hide",
    subject: "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.comment/3kabc",
    value: "hidden",
    atMs: 1_700_000_000_000,
    status: "effective",
    lapsed: false,
    ...over,
  };
}

async function setup(engine = new MockEngine({ holdMs: 0, sendMs: 0 })) {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
  delete view.viewer;
  renderWithApp(
    <ForumContext value={view}>
      <ForumModPage />
    </ForumContext>,
    { engine },
  );
  return { engine, forum };
}

async function acknowledge(): Promise<void> {
  const ack = await screen.findByRole("dialog", { name: "Moderation actions are public" });
  fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
}

describe("ForumModPage", () => {
  it("pages through the mod log with the engine's cursor, rows keyed by uri", async () => {
    const engine = new MockEngine();
    const twin = { atMs: 5, subject: "at://did:plc:a/c/1", action: "hide" };
    const modLog = vi.spyOn(engine, "modLog").mockImplementation(async (_forum, cursor) =>
      cursor === undefined
        ? {
            dto: DTO_VERSION,
            entries: [
              entry({
                ...twin,
                uri: "at://did:plc:m/online.7778777.lessdugin.temp.modAction/a",
                value: "first",
              }),
              entry({
                ...twin,
                uri: "at://did:plc:m/online.7778777.lessdugin.temp.modAction/b",
                value: "second",
              }),
            ],
            cursor: "c1",
          }
        : { dto: DTO_VERSION, entries: [entry({ value: "older" })] },
    );
    const { forum } = await setup(engine);
    // two entries with the same time, subject and action are two rows
    expect(await screen.findByText(/hide: first/)).toBeTruthy();
    expect(screen.getByText(/hide: second/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByText(/hide: older/)).toBeTruthy();
    expect(modLog).toHaveBeenLastCalledWith(forum, "c1");
    await waitFor(() => expect(screen.queryByRole("button", { name: "Load more" })).toBeNull());
  });

  it("recognises the queue's blocked answer by its code, not its wording", () => {
    const blocked = { code: "blocked", message: "the mod queue is for the forum's moderators" };
    expect(isBlocked({ ...blocked, retryable: false })).toBe(true);
    expect(
      isBlocked({ code: "network", message: "authorities unreachable", retryable: true }),
    ).toBe(false);
  });

  it("acts on queue items: rejects an applicant, keeps a newcomer's item with the reason", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const newcomerItem =
      "at://did:plc:newnewnewnewnewnewnewnewn/online.7778777.lessdugin.temp.post/3k";
    vi.spyOn(engine, "modQueue").mockResolvedValue({
      dto: DTO_VERSION,
      items: [
        {
          kind: "applicant",
          subject: "did:plc:applicantapplicantapplic",
          reason: "applied to join",
        },
        { kind: "newcomer", subject: newcomerItem, reason: "a new user's post" },
      ],
    });
    const reject = vi.spyOn(engine, "reject").mockResolvedValue([]);
    const mod = vi
      .spyOn(engine, "modAction")
      .mockResolvedValue({ id: "w1", state: { kind: "sending" } });
    const { forum } = await setup(engine);
    fireEvent.click(await screen.findByRole("button", { name: "Reject" }));
    await waitFor(() =>
      expect(reject).toHaveBeenCalledWith(forum, ["did:plc:applicantapplicantapplic"]),
    );
    fireEvent.change(screen.getByRole("textbox", { name: /Reason for the actions below/ }), {
      target: { value: "looks fine" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Keep" }));
    await acknowledge();
    await waitFor(() =>
      expect(mod).toHaveBeenCalledWith({
        kind: "assert",
        forum,
        scope: "forum",
        subject: newcomerItem,
        action: { kind: "hide", hidden: false, spam: false, showReason: true },
        reason: "looks fine",
      }),
    );
  });

  it("removes the viewer's own assertion from the log", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const me = (await engine.activeAccount())?.did ?? "";
    const comment =
      "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.comment/3kabc";
    vi.spyOn(engine, "modLog").mockResolvedValue({
      dto: DTO_VERSION,
      entries: [
        entry({
          uri: `at://${me}/online.7778777.lessdugin.temp.modAction/k-1`,
          actor: { did: me, shortDid: "me", handleState: "unverified" },
          action: "pin",
          subject: comment,
          value: "pinned",
        }),
        entry({}),
      ],
    });
    const mod = vi
      .spyOn(engine, "modAction")
      .mockResolvedValue({ id: "w1", state: { kind: "sending" } });
    const { forum } = await setup(engine);
    const remove = await screen.findAllByRole("button", { name: "Remove" });
    // only the viewer's own entry
    expect(remove).toHaveLength(1);
    fireEvent.click(remove[0] as HTMLElement);
    await acknowledge();
    await waitFor(() =>
      expect(mod).toHaveBeenCalledWith({
        kind: "remove",
        forum,
        scope: "forum",
        subject: comment,
        actionKey: "pin:thread",
      }),
    );
  });

  it("adopts a former moderator's assertions", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const adopt = vi.spyOn(engine, "modAdopt");
    const { forum } = await setup(engine);
    fireEvent.change(await screen.findByRole("textbox", { name: /Former moderator/ }), {
      target: { value: OTHER },
    });
    fireEvent.click(screen.getByRole("button", { name: "Adopt" }));
    expect(adopt).not.toHaveBeenCalled();
    await acknowledge();
    await waitFor(() => expect(adopt).toHaveBeenCalledWith(forum, OTHER));
  });

  it("approves an applicant from the queue only after the interstitial (m3-4, sec-4)", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const applicant = "did:plc:applicantapplicantapplic";
    vi.spyOn(engine, "modQueue").mockResolvedValue({
      dto: DTO_VERSION,
      items: [{ kind: "applicant", subject: applicant, reason: "applied to join" }],
    });
    const approve = vi.spyOn(engine, "approve").mockResolvedValue([]);
    const { forum } = await setup(engine);
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
    expect(await screen.findByRole("dialog", { name: "Moderation actions are public" })).toBeTruthy();
    expect(approve).not.toHaveBeenCalled();
    await acknowledge();
    await waitFor(() => expect(approve).toHaveBeenCalledWith(forum, [applicant]));
  });
});
