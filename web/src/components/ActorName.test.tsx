// SPDX-License-Identifier: AGPL-3.0-or-later
// A byline always shows the verified handle or the short DID; a display name is never shown
// alone (threat-model ID-5, review finding sec-11).
import type { ActorView } from "@lessdugin/engine/types";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ActorName } from "./ActorName.tsx";

const impostor: ActorView = {
  did: "did:plc:abcdefghijklmnopqrstuvwx",
  shortDid: "did:plc:abcd…uvwx",
  handleState: "unverified",
  displayName: "@alice.bsky.social",
};

describe("ActorName", () => {
  it("shows the short DID next to a display name when the handle is not verified", () => {
    render(<ActorName actor={impostor} link={false} />);
    expect(screen.getByText("@alice.bsky.social")).toBeTruthy();
    expect(screen.getByText("did:plc:abcd…uvwx")).toBeTruthy();
  });

  it("shows the verified handle next to a display name", () => {
    render(
      <ActorName
        actor={{ ...impostor, handleState: "verified", handle: "mallory.example" }}
        link={false}
      />,
    );
    expect(screen.getByText("@mallory.example")).toBeTruthy();
  });
});
