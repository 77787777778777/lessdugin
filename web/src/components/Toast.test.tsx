// SPDX-License-Identifier: AGPL-3.0-or-later
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ToastRegion, toasts } from "./Toast.tsx";

afterEach(() => {
  for (const t of toasts.snapshot()) toasts.dismiss(t.id);
  vi.useRealTimers();
});

describe("toasts (WCAG 2.2.1)", () => {
  it("pause while hovered or focused, then dismiss with the remaining time", () => {
    vi.useFakeTimers();
    render(<ToastRegion />);
    act(() => {
      toasts.show({
        message: "Vote recorded",
        durationMs: 1000,
        action: { label: "Undo", onAction: () => {} },
      });
    });
    const toast = screen.getByText("Vote recorded").parentElement as HTMLElement;
    act(() => vi.advanceTimersByTime(600));
    fireEvent.pointerEnter(toast);
    act(() => vi.advanceTimersByTime(5000));
    expect(screen.queryByText("Vote recorded")).not.toBeNull();
    fireEvent.pointerLeave(toast);
    fireEvent.focusIn(screen.getByRole("button", { name: "Undo" }));
    act(() => vi.advanceTimersByTime(5000));
    expect(screen.queryByText("Vote recorded")).not.toBeNull();
    fireEvent.focusOut(screen.getByRole("button", { name: "Undo" }));
    act(() => vi.advanceTimersByTime(300));
    expect(screen.queryByText("Vote recorded")).not.toBeNull();
    act(() => vi.advanceTimersByTime(200));
    expect(screen.queryByText("Vote recorded")).toBeNull();
  });

  it("evicts plain messages before one that offers an action", () => {
    render(<ToastRegion />);
    act(() => {
      toasts.show({ message: "undoable", action: { label: "Undo", onAction: () => {} } });
      for (let i = 0; i < 6; i++) toasts.show({ message: `info ${i}` });
    });
    expect(screen.queryByText("undoable")).not.toBeNull();
    expect(toasts.snapshot()).toHaveLength(4);
    expect(screen.queryByText("info 5")).not.toBeNull();
    expect(screen.queryByText("info 0")).toBeNull();
  });
});
