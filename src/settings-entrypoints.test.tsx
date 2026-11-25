import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { setupApp } from "./test-utils";
import { createCommands } from "./app/commands";
import { defaultSettings } from "./app/state";
afterEach(cleanup);
test("gear, Ctrl+, and palette open the native window; inline remains the fallback", async () => {
  const app = await setupApp();
  const open = vi.spyOn(app.api, "settingsWindowOpen").mockResolvedValue("opened");
  await app.user.click(screen.getByRole("button", { name: /^Einstellungen/ }));
  await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
  expect(app.runtime.store.getState().dialog).toBeNull();
  fireEvent.keyDown(document, { key: ",", ctrlKey: true });
  await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
  createCommands(app.runtime.store.getState(), app.runtime.controller, key => key, () => {}).find(c => c.id === "settings")!.execute();
  await waitFor(() => expect(open).toHaveBeenCalledTimes(3));
  open.mockResolvedValue("inline");
  await app.runtime.controller.openSettings("info");
  expect(app.runtime.store.getState().dialog).toEqual({ kind: "settings", tab: "info" });
  app.runtime.controller.dispose();
});
test("the settings window opens with the remembered size unless the mode is never or nothing is remembered", async () => {
  const sizes = [
    [{ settingsWindowSizeMode: "ask", settingsWindowSize: { width: 900, height: 700 } }, { width: 900, height: 700 }],
    [{ settingsWindowSizeMode: "always", settingsWindowSize: { width: 900, height: 700 } }, { width: 900, height: 700 }],
    [{ settingsWindowSizeMode: "never", settingsWindowSize: { width: 900, height: 700 } }, null],
    [{ settingsWindowSizeMode: "ask", settingsWindowSize: { width: 0, height: 0 } }, null],
  ] as const;
  for (const [settings, expected] of sizes) {
    const app = await setupApp({ settings: { ...defaultSettings, ...settings } });
    const open = vi.spyOn(app.api, "settingsWindowOpen").mockResolvedValue("opened");
    await app.runtime.controller.openSettings();
    expect(open.mock.calls[0]![3]).toEqual(expected);
    app.runtime.controller.dispose();
    cleanup();
  }
});
