import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime } from "../app/runtime";
import { createMockApi } from "../ipc/mock";
import type { WinIdx } from "../app/state";
import { QuickAccessBar } from "./QuickAccessBar";

async function setup(showQuickAccess: boolean, w: WinIdx = 0) {
  const api = createMockApi({
    initialSettings: {
      version: 1, verifyPolicy: "auto", theme: "schiefer", accent: "#e0a45c", density: "normal", fontSize: 13, language: "en",
      showQuickAccess,
      bookmarks: [{ name: "Docs", path: "/home/demo/Docs", color: "#aabbcc", order: 0 }],
      mirror: false, session: { windows: [], activeWindow: 0 },
    },
  });
  const rt = createRuntime(api, { toastMs: 50 });
  await rt.controller.start();
  const nav = vi.spyOn(rt.controller, "navigatePlace").mockResolvedValue(true);
  render(<RuntimeProvider runtime={rt}><QuickAccessBar w={w} /></RuntimeProvider>);
  return { rt, nav };
}

describe("QuickAccessBar", () => {
  test("renders nothing when disabled", async () => {
    await setup(false);
    expect(screen.queryByRole("navigation")).toBeNull();
  });

  test("shows bookmarks, home and drives; click navigates its own pane", async () => {
    const { nav } = await setup(true, 0);
    const user = userEvent.setup();
    expect(await screen.findByRole("button", { name: "USB-Stick" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Docs" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Sicherung" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Home folder" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "USB-Stick" }));
    expect(nav).toHaveBeenCalledWith(0, "/run/media/demo/USB-Stick");
    await user.click(screen.getByRole("button", { name: "Docs" }));
    expect(nav).toHaveBeenLastCalledWith(0, "/home/demo/Docs");
    await user.click(screen.getByRole("button", { name: "Home folder" }));
    expect(nav).toHaveBeenLastCalledWith(0, "/home/demo");
  });

  test("navigates the pane it belongs to (w=1)", async () => {
    const { nav } = await setup(true, 1);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Sicherung" }));
    expect(nav).toHaveBeenCalledWith(1, "/run/media/demo/Sicherung");
  });
});
