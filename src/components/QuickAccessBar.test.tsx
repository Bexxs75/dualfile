import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime } from "../app/runtime";
import { createMockApi } from "../ipc/mock";
import { QuickAccessBar } from "./QuickAccessBar";

async function setup(showQuickAccess: boolean) {
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
  render(<RuntimeProvider runtime={rt}><QuickAccessBar /></RuntimeProvider>);
  return { rt, nav };
}

describe("QuickAccessBar", () => {
  test("renders nothing when disabled", async () => {
    await setup(false);
    expect(screen.queryByRole("navigation")).toBeNull();
  });

  test("shows bookmarks and drives; click navigates the active pane", async () => {
    const { nav } = await setup(true);
    const user = userEvent.setup();
    expect(await screen.findByRole("button", { name: "Docs" })).toBeTruthy();
    expect(await screen.findByRole("button", { name: "USB-Stick" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Sicherung" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "USB-Stick" }));
    expect(nav).toHaveBeenCalledWith(0, "/run/media/demo/USB-Stick");
    await user.click(screen.getByRole("button", { name: "Docs" }));
    expect(nav).toHaveBeenLastCalledWith(0, "/home/demo/Docs");
  });

  test("uses the active pane after switching", async () => {
    const { rt, nav } = await setup(true);
    const user = userEvent.setup();
    rt.controller.setActive(1);
    await waitFor(() => expect(rt.store.getState().active).toBe(1));
    await user.click(await screen.findByRole("button", { name: "Sicherung" }));
    expect(nav).toHaveBeenCalledWith(1, "/run/media/demo/Sicherung");
  });
});
