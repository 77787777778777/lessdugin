// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// The moderation dialog (review findings sec-5, m3-9): a ban and a restriction say they are
// retroactive before they are sent and show the engine's notice after; restrict and nullify
// exist; a moderator undoes their own action by removing their own record; a post's author gets
// only the author-scope actions of social.md §4.9.

import { MockEngine } from "@lessdugin/engine/mock";
import type { ActionTicket, ModActionIntent } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { ModMenu, type ModSubject, modChoices, RETROACTIVE } from "./ModMenu.tsx";

const FORUM = "at://did:plc:ownerownerownerownerowne/online.7778777.lessdugin.temp.forum/f";
const SUBJECT: ModSubject = {
  kind: "comment",
  uri: "at://did:plc:authorauthorauthorauthor/online.7778777.lessdugin.temp.comment/3kabc",
  cid: "bafyreiexample",
  author: "did:plc:authorauthorauthorauthor",
  pinned: false,
};

function setup(ticket: ActionTicket = { id: "w1", state: { kind: "sending" } }) {
  const engine = new MockEngine();
  const modAction = vi.spyOn(engine, "modAction").mockResolvedValue(ticket);
  renderWithApp(<ModMenu forum={FORUM} subject={SUBJECT} canCurate={false} />, { engine });
  return { modAction };
}

async function choose(label: string | RegExp): Promise<HTMLElement> {
  fireEvent.click(await screen.findByRole("button", { name: "Moderate" }));
  const dialog = await screen.findByRole("dialog", { name: "Moderate this comment" });
  fireEvent.click(within(dialog).getByRole("radio", { name: label }));
  return dialog;
}

async function apply(dialog: HTMLElement): Promise<void> {
  fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
  const ack = await screen.findByRole("dialog", { name: "Moderation actions are public" });
  fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
}

const sent = (spy: { mock: { calls: unknown[][] } }): ModActionIntent =>
  spy.mock.calls[0]?.[0] as ModActionIntent;

describe("ModMenu", () => {
  it("says a ban is retroactive before sending it, and shows the engine's notice after", async () => {
    const { modAction } = setup({
      id: "w1",
      state: { kind: "sending" },
      notices: [{ code: "retroactive", message: "a ban is retroactive (engine)" }],
    });
    const dialog = await choose("Ban the author from this forum");
    expect(within(dialog).getByTestId("mod-retroactive").textContent).toBe(RETROACTIVE.ban);
    await apply(dialog);
    await waitFor(() => expect(modAction).toHaveBeenCalledTimes(1));
    expect(sent(modAction)).toMatchObject({
      kind: "assert",
      subject: SUBJECT.author,
      action: { kind: "access", level: "banned" },
    });
    expect(await screen.findByText("a ban is retroactive (engine)")).toBeTruthy();
  });

  it("restricts with the chosen capabilities only, so a nullification stays (m3r-2)", async () => {
    const { modAction } = setup();
    const dialog = await choose("Restrict the author");
    expect(within(dialog).getByTestId("mod-retroactive").textContent).toBe(RETROACTIVE.restrict);
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "posts" }));
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "votes" }));
    await apply(dialog);
    await waitFor(() => expect(modAction).toHaveBeenCalledTimes(1));
    expect(sent(modAction)).toMatchObject({
      kind: "assert",
      scope: "forum",
      subject: SUBJECT.author,
    });
    // `nullifyVotes` left out: the engine keeps the moderator's current value of it
    expect(sent(modAction)).toHaveProperty("action", {
      kind: "restrict",
      capabilities: ["comment", "vote"],
    });
  });

  it.each([
    [/^Nullify the author's votes/, true],
    ["Let the author's votes count again", false],
  ])("%s sends only nullifyVotes, so a restriction stays (m3r-2)", async (label, nullifyVotes) => {
    const { modAction } = setup();
    const dialog = await choose(label);
    expect(within(dialog).queryByRole("checkbox", { name: "posts" })).toBeNull();
    await apply(dialog);
    await waitFor(() => expect(modAction).toHaveBeenCalledTimes(1));
    expect(sent(modAction)).toMatchObject({ kind: "assert", subject: SUBJECT.author });
    expect(sent(modAction)).toHaveProperty("action", { kind: "restrict", nullifyVotes });
  });

  it("unbans by removing the moderator's own #access record", async () => {
    const { modAction } = setup();
    const dialog = await choose(/Remove my ban of the author/);
    await apply(dialog);
    await waitFor(() => expect(modAction).toHaveBeenCalledTimes(1));
    expect(sent(modAction)).toEqual({
      kind: "remove",
      forum: FORUM,
      scope: "forum",
      subject: SUBJECT.author,
      actionKey: "access",
    });
  });

  it("removes the own thread pin of a comment with its action key", async () => {
    const { modAction } = setup();
    const dialog = await choose("Remove my pin");
    await apply(dialog);
    await waitFor(() => expect(modAction).toHaveBeenCalledTimes(1));
    expect(sent(modAction)).toMatchObject({ kind: "remove", actionKey: "pin:thread" });
  });

  it("offers a post's author only the author-scope actions (social.md §4.9)", () => {
    expect(modChoices(SUBJECT, "author", false)).toEqual([
      "hide",
      "ban",
      "pin",
      "removeHide",
      "removeAccess",
      "removePin",
    ]);
    expect(modChoices({ ...SUBJECT, kind: "post" }, "author", false)).toEqual([
      "lock",
      "removeLock",
    ]);
    expect(modChoices({ ...SUBJECT, kind: "post" }, "forum", false)).not.toContain("tierCurated");
  });
});
