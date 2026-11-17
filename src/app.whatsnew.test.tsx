import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { setupApp } from "./test-utils";
import type { Settings } from "./ipc/types";
import { currentVersion } from "./release-notes";
import { createRuntime } from "./app/runtime";
import { createMockApi } from "./ipc/mock";
import { defaultSettings } from "./app/state";
import { createCommands } from "./app/commands";
import { createTranslator } from "./i18n";

test.each(["de", "en"] as const)("automatic notes render in %s, focus Got it, and open the release via IPC", async language => {
  const openUrl = vi.fn(async () => {});
  const app = await setupApp({ language, settings: { lastSeenVersion: null }, wrap: api => ({ ...api, openUrl }) });
  const heading = language === "de" ? `Neu in Dualfile ${currentVersion}` : `What’s new in Dualfile ${currentVersion}`;
  expect(screen.getByRole("dialog", { name: heading })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: language === "de" ? "Alles klar" : "Got it" })).toHaveFocus();
  await app.user.click(screen.getByRole("link", { name: /Changelog|changelog/ }));
  expect(openUrl).toHaveBeenCalledWith(`https://github.com/Bexxs75/dualfile/releases/tag/v${currentVersion}`);
});

test.each(["button", "escape", "x"])("%s closes and persists only after acknowledgment", async close => {
  const saveSettings = vi.fn(async (_settings: Settings) => {});
  const app = await setupApp({ settings: { lastSeenVersion: null }, wrap: api => ({ ...api, saveSettings }) });
  await act(async () => { await app.runtime.controller.saveNow(); });
  expect(saveSettings.mock.calls.every(call => call[0].lastSeenVersion !== currentVersion)).toBe(true);
  expect(app.runtime.store.getState().settings.lastSeenVersion).toBeNull();
  if (close === "escape") await app.user.keyboard("{Escape}");
  else await app.user.click(screen.getByRole("button", { name: close === "x" ? "Schließen" : "Alles klar" }));
  await waitFor(() => expect(saveSettings).toHaveBeenLastCalledWith(expect.objectContaining({ lastSeenVersion: currentVersion })));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("fresh install saves current version without showing notes; next start stays quiet", async () => {
  const api = createMockApi();
  const first = createRuntime(api);
  await first.controller.start();
  expect(first.store.getState().dialog).toBeNull();
  await first.controller.saveNow();
  expect((await api.loadSettings()).settings.lastSeenVersion).toBe(currentVersion);
  first.controller.dispose();
  const second = createRuntime(api);
  await second.controller.start();
  expect(second.store.getState().dialog).toBeNull();
  second.controller.dispose();
});

test("palette reopens current notes even after downgrade and does not lower the marker", async () => {
  const app = await setupApp({ settings: { lastSeenVersion: "0.3.6" } });
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.keyDown(document.activeElement!, { key: "k", ctrlKey: true });
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "Neuigkeiten" } });
  await app.user.click(screen.getByRole("option", { name: /Neuigkeiten/ }));
  expect(screen.getByRole("dialog", { name: `Neu in Dualfile ${currentVersion}` })).toBeInTheDocument();
  await app.user.keyboard("{Escape}");
  expect(app.runtime.store.getState().settings.lastSeenVersion).toBe("0.3.6");
});

test("menu reopens notes and skipped versions have their own headings", async () => {
  const app = await setupApp({ settings: { lastSeenVersion: "0.3.2" } });
  expect(screen.getByText(/mehrere Versionen/)).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: /0.3.3/ })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: new RegExp(currentVersion) })).toBeInTheDocument();
  await app.user.keyboard("{Escape}");
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Neuigkeiten" }));
  expect(screen.getByRole("dialog", { name: `Neu in Dualfile ${currentVersion}` })).toBeInTheDocument();
  expect(screen.queryByText(/mehrere Versionen/)).toBeNull();
});

test("newer-version read-only mode suppresses notes and writes, including manual opening", async () => {
  const base = createMockApi();
  const saveSettings = vi.fn(base.saveSettings);
  const runtime = createRuntime({ ...base, saveSettings, loadSettings: async () => ({ settings: defaultSettings, outcome: { kind: "newerVersion", version: 99 } }) });
  await runtime.controller.start();
  runtime.controller.openWhatsNew();
  expect(runtime.store.getState().dialog).toBeNull();
  const command = createCommands(runtime.store.getState(), runtime.controller, createTranslator("en"), () => {}).find(c => c.id === "whatsNew")!;
  expect(command.enabled(runtime.store.getState())).toBe(false);
  await runtime.controller.saveNow();
  expect(saveSettings).not.toHaveBeenCalled();
  runtime.controller.dispose();
});
