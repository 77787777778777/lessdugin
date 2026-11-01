// The attachment row (docs/plan.md M4): a file is prepared through the engine, and a refusal is
// shown with the engine's own reason rather than a generic failure.

import type { MediaRef } from "@lessdugin/engine/types";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { type Attachment, Attachments, publishableName } from "./Attachments.tsx";

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

  it("ignores a file the media pipeline cannot take", async () => {
    engine.prepareMedia.mockClear();
    const onChange = vi.fn();
    render(<Attachments value={[]} onChange={onChange} />);
    const txt = new File(["hello"], "notes.txt", { type: "text/plain" });
    fireEvent.change(screen.getByTestId("attachment-input"), {
      target: { files: [txt] },
    });
    expect(engine.prepareMedia).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("takes the pipeline's other types too: video, audio, PDF (m4-20)", async () => {
    engine.prepareMedia.mockClear();
    engine.prepareMedia.mockResolvedValue(ref({ mime: "application/pdf" }));
    const onChange = vi.fn();
    render(<Attachments value={[]} onChange={onChange} />);
    const pdf = new File(["%PDF-1.7"], "paper.pdf", { type: "application/pdf" });
    const mp3 = new File([new Uint8Array([0x49, 0x44, 0x33])], "song.mp3", { type: "audio/mpeg" });
    fireEvent.change(screen.getByTestId("attachment-input"), { target: { files: [pdf, mp3] } });
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(engine.prepareMedia).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId("attachment-input").getAttribute("accept")).toContain(
      "application/pdf",
    );
  });

  it("publishes a file name only when the author opts in (m4-28)", async () => {
    engine.prepareMedia.mockClear();
    engine.prepareMedia.mockResolvedValue(ref());
    const onChange = vi.fn();
    const { rerender } = render(<Attachments value={[]} onChange={onChange} />);
    fireEvent.change(screen.getByTestId("attachment-input"), {
      target: { files: [png("IMG_20261026_142233.jpg")] },
    });
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    // the name never reaches the engine, so it is not in the prepared reference
    const hints = engine.prepareMedia.mock.calls[0]?.[1] as Record<string, unknown>;
    expect(hints).not.toHaveProperty("name");
    const rows = onChange.mock.calls.at(-1)?.[0] as Attachment[];
    const row = rows[0];
    if (row?.kind !== "ready") throw new Error("not ready");
    expect(row.ref.name).toBeUndefined();

    rerender(<Attachments value={rows} onChange={onChange} />);
    const optIn = screen.getByRole("checkbox", { name: /Publish the file name/ });
    expect((optIn as HTMLInputElement).checked).toBe(false);
    fireEvent.click(optIn);
    const named = onChange.mock.calls.at(-1)?.[0] as Attachment[];
    expect(named[0]?.kind === "ready" && named[0].ref.name).toBe("IMG_20261026_142233.jpg");

    rerender(<Attachments value={named} onChange={onChange} />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Publish the file name/ }));
    const unnamed = onChange.mock.calls.at(-1)?.[0] as Attachment[];
    expect(unnamed[0]?.kind === "ready" && "name" in unnamed[0].ref).toBe(false);
  });

  it("offers no name readers would ignore (data-model.md §4)", () => {
    expect(publishableName("chart.png")).toBe("chart.png");
    expect(publishableName("a/b.png")).toBeNull();
    expect(publishableName("a\\b.png")).toBeNull();
    expect(publishableName("bell\u0007.png")).toBeNull();
    expect(publishableName("c1\u0085.png")).toBeNull();
    expect(publishableName("line\u2028.png")).toBeNull();
    expect(publishableName(`${"é".repeat(128)}.png`)).toBeNull();
    expect(publishableName("")).toBeNull();
  });

  it("warns about missing alt text until the author writes one (m4-19)", () => {
    const notices = [{ code: "missingAlt" as const, message: "this file has no alt text" }];
    const row = (alt: string): Attachment[] => [
      { kind: "ready", name: "photo.jpg", ref: ref({ alt, notices }) },
    ];
    const { rerender } = render(<Attachments value={row("")} onChange={() => {}} />);
    expect(screen.getByTestId("missing-alt").textContent).toBe("this file has no alt text");
    rerender(<Attachments value={row("   ")} onChange={() => {}} />);
    expect(screen.getByTestId("missing-alt")).toBeTruthy();
    rerender(<Attachments value={row("A bar chart")} onChange={() => {}} />);
    expect(screen.queryByTestId("missing-alt")).toBeNull();
  });
});
