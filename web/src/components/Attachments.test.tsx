// The attachment row (docs/plan.md M4): a file is prepared through the engine, and a refusal is
// shown with the engine's own reason rather than a generic failure.

import type { MediaRef } from "@lessdugin/engine/types";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Attachments } from "./Attachments.tsx";

const engine = vi.hoisted(() => ({
  prepareMedia: vi.fn(),
}));

vi.mock("../engine/context.tsx", () => ({
  useEngine: () => engine,
}));

const ref = (over: Partial<MediaRef> = {}): MediaRef => ({
  id: "bafkrei-example",
  cid: "bafkrei-example" as MediaRef["cid"],
  mime: "image/jpeg",
  sizeBytes: 1024,
  alt: "",
  ...over,
});

function png(name = "photo.jpg"): File {
  return new File([new Uint8Array([0xff, 0xd8, 0xff])], name, { type: "image/jpeg" });
}

describe("Attachments", () => {
  it("prepares a picked file and shows it as ready", async () => {
    engine.prepareMedia.mockResolvedValue(ref());
    const onChange = vi.fn();
    render(<Attachments value={[]} onChange={onChange} />);

    fireEvent.change(screen.getByTestId("attachment-input"), {
      target: { files: [png()] },
    });
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const rows = onChange.mock.calls.at(-1)?.[0] as { kind: string }[];
    expect(rows).toHaveLength(1);
    expect(rows[0]?.kind).toBe("ready");
    expect(engine.prepareMedia).toHaveBeenCalledTimes(1);
  });

  it("shows the engine reason when a file is refused", async () => {
    engine.prepareMedia.mockRejectedValue(
      new Error("image is too large: 9.2 MB, the limit is 5 MB"),
    );
    const onChange = vi.fn();
    render(<Attachments value={[]} onChange={onChange} />);

    fireEvent.change(screen.getByTestId("attachment-input"), {
      target: { files: [png("big.jpg")] },
    });
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const rows = onChange.mock.calls.at(-1)?.[0] as { kind: string; reason: string }[];
    expect(rows[0]?.kind).toBe("refused");
    expect(rows[0]?.reason).toContain("the limit is 5 MB");
  });

  it("ignores a file that is not an image", async () => {
    const onChange = vi.fn();
    render(<Attachments value={[]} onChange={onChange} />);
    const txt = new File(["hello"], "notes.txt", { type: "text/plain" });
    fireEvent.change(screen.getByTestId("attachment-input"), {
      target: { files: [txt] },
    });
    expect(engine.prepareMedia).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  });
});
