import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { setupApp } from "./test-utils";
import type { UpdateProgress } from "./ipc/system";
import config from "../src-tauri/tauri.conf.json";

const available = { version: "0.2.0", notes: "Fixed files\n<script>unsafe()</script>" };
const settingsDialog = async (app: Awaited<ReturnType<typeof setupApp>>) => {
  act(() => app.runtime.controller.openDialog({ kind: "settings" }));
  return within(await screen.findByRole("dialog", { name: "Einstellungen" }));
};

test("settings groups: immediate tray, fallback hint, entry actions, version, manual status and errors", async () => {
  const tray = vi.fn(async () => false);
  const menu = vi.fn(async () => {});
  const check = vi.fn(async () => null as typeof available | null);
  const app = await setupApp({ settings: { checkUpdatesOnStart: false }, wrap: api => ({ ...api, traySet: tray, startMenuSet: menu, updateCheck: check }) });
  expect(check).not.toHaveBeenCalled();
  const dlg = await settingsDialog(app);
  expect(dlg.getByRole("region", { name: "System" })).toBeInTheDocument();
  // Reads the version from the same source the dialog does, so a version bump never breaks this test.
  expect(dlg.getByRole("region", { name: "Updates" })).toHaveTextContent(`Installierte Version ${config.version}`);
  await app.user.click(dlg.getByRole("checkbox", { name: /Symbol im Infobereich/ }));
  expect(tray).toHaveBeenLastCalledWith(true, "de");
  expect(app.runtime.store.getState().settings.trayEnabled).toBe(true);
  expect(dlg.getByText(/Kein Infobereich gefunden/)).toBeInTheDocument();
  await app.user.click(dlg.getByRole("button", { name: "Eintrag entfernen" }));
  expect(menu).toHaveBeenLastCalledWith(false);
  await app.user.click(dlg.getByRole("button", { name: "Eintragen" }));
  expect(menu).toHaveBeenLastCalledWith(true);
  await app.user.click(dlg.getByRole("button", { name: "Jetzt prüfen" }));
  expect(dlg.getByText(/Zuletzt geprüft:/)).toHaveTextContent("aktuell");
  check.mockRejectedValueOnce(new Error("Offline"));
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  await app.user.click(dlg.getByRole("button", { name: "Jetzt prüfen" }));
  expect(dlg.getByRole("alert")).toHaveTextContent("Offline");
  warn.mockRestore();
});

test("banner: notes are plain text, Escape restores focus, Later persists and manual check restores banner", async () => {
  const check = vi.fn(async () => available);
  const install = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: check, updateInstall: install }) });
  const banner = within(await screen.findByRole("region", { name: "Update verfügbar" }));
  const notes = banner.getByRole("button", { name: "Änderungen" });
  await app.user.click(notes);
  const dlg = screen.getByRole("dialog", { name: "Änderungen · 0.2.0" });
  expect(dlg).toHaveTextContent("<script>unsafe()</script>");
  expect(dlg.querySelector("script")).toBeNull();
  await app.user.keyboard("{Escape}");
  expect(notes).toHaveFocus();
  await app.user.click(banner.getByRole("button", { name: "Später" }));
  expect(screen.queryByRole("region", { name: "Update verfügbar" })).toBeNull();
  expect(app.runtime.store.getState().settings.dismissedUpdateVersion).toBe("0.2.0");
  await act(async () => app.runtime.system.check());
  expect(screen.queryByRole("region", { name: "Update verfügbar" })).toBeNull();
  const settings = await settingsDialog(app);
  await app.user.click(settings.getByRole("button", { name: "Jetzt prüfen" }));
  expect(screen.getByRole("region", { name: "Update verfügbar" })).toBeInTheDocument();
  expect(install).not.toHaveBeenCalled();
  expect(check).toHaveBeenCalledTimes(3);
});

test("without AppImage the only update action opens the release page", async () => {
  const open = vi.fn(async () => {}), install = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, updateInstall: install, releaseOpen: open }) });
  await app.user.click(await screen.findByRole("button", { name: "Release-Seite öffnen" }));
  expect(open).toHaveBeenCalledOnce();
  expect(install).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Aktualisieren" })).toBeNull();
});

test("running and paused jobs disable installation with an explanation, completion unlocks", async () => {
  const install = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true }), updateInstall: install }) });
  const button = await screen.findByRole("button", { name: "Aktualisieren" });
  // A real mock transfer supplies a full job DTO; keep it queued via the manual scheduler.
  await act(async () => { await app.api.copy(["/home/demo/Dokumente/notizen.txt"], "/home/demo/Downloads"); });
  await waitFor(() => expect(button).toBeDisabled());
  expect(screen.getByText("Erst nach Abschluss der Aufträge.")).toBeInTheDocument();
  await app.user.click(button);
  expect(install).not.toHaveBeenCalled();
  const job = (await app.api.jobs())[0]!;
  await act(async () => { await app.api.jobPause(job.id); });
  expect(button).toBeDisabled();
  await act(async () => { await app.api.jobResume(job.id); });
  await app.runJobs();
  await waitFor(() => expect(button).toBeEnabled());
});

test.each([false, true])("first-start refusal, never=%s, persists count and choice", async never => {
  const app = await setupApp({ settings: { checkUpdatesOnStart: false }, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false }) }) });
  const dlg = within(await screen.findByRole("dialog", { name: "Dualfile im Startmenü eintragen?" }));
  expect(app.runtime.store.getState().settings.startMenuPromptCount).toBe(1);
  if (never) await app.user.click(dlg.getByRole("checkbox", { name: "Nicht noch einmal fragen" }));
  await app.user.click(dlg.getByRole("button", { name: "Nein, danke" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(app.runtime.store.getState().settings.startMenuPrompted).toBe(never);
  expect((await app.api.loadSettings()).settings.startMenuPrompted).toBe(never);
});

test("third prompt: Escape counts as refusal and no fourth prompt appears", async () => {
  const app = await setupApp({ settings: { startMenuPromptCount: 2 }, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false }) }) });
  await screen.findByRole("dialog", { name: "Dualfile im Startmenü eintragen?" });
  await app.user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(app.runtime.store.getState().settings.startMenuPromptCount).toBe(3);
  expect(app.runtime.store.getState().settings.startMenuPrompted).toBe(true);
  app.view.unmount();
  await setupApp({ settings: { startMenuPromptCount: 3, startMenuPrompted: false }, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false }) }) });
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("first-start add retries a failed entry and closes after success", async () => {
  const menu = vi.fn(async () => {}).mockRejectedValueOnce(new Error("Read only"));
  const app = await setupApp({ wrap: api => ({ ...api, startMenuSet: menu, systemStatus: async () => ({ appImage: false, startMenuExists: false }) }) });
  const dlg = within(await screen.findByRole("dialog", { name: "Dualfile im Startmenü eintragen?" }));
  await app.user.click(dlg.getByRole("button", { name: "Eintragen" }));
  expect(dlg.getByRole("alert")).toHaveTextContent("Read only");
  await app.user.click(dlg.getByRole("button", { name: "Eintragen" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(menu).toHaveBeenLastCalledWith(true);
  expect(app.runtime.store.getState().settings.startMenuPrompted).toBe(true);
});

test("progress cancellation calls the native boundary and does not restart", async () => {
  let reject!: (error: Error) => void;
  const restart = vi.fn(async () => {});
  const cancel = vi.fn(async () => { reject(new Error("Update cancelled")); return true; });
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true }), restart, updateCancel: cancel,
    updateInstall: async (_v, progress) => { progress({ downloaded: 41e6, total: 66e6, phase: "download" }); await new Promise<void>((_resolve, fail) => { reject = fail; }); },
  }) });
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  const dlg = within(await screen.findByRole("dialog", { name: "Aktualisieren auf 0.2.0" }));
  expect(dlg.getByRole("progressbar")).toHaveAttribute("value", "62");
  await app.user.click(dlg.getByRole("button", { name: "Abbrechen" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(cancel).toHaveBeenCalledOnce(); expect(restart).not.toHaveBeenCalled();
  expect(screen.queryByRole("alert")).toBeNull();
});

test("install phase blocks Escape/cancel; restart failure offers retry", async () => {
  let report!: (p: UpdateProgress) => void, finish!: () => void;
  const restart = vi.fn(async () => {}).mockRejectedValueOnce(new Error("Restart failed"));
  const cancel = vi.fn(async () => true);
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true }), restart, updateCancel: cancel,
    updateInstall: async (_v, progress) => { report = progress; await new Promise<void>(resolve => { finish = resolve; }); },
  }) });
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  await waitFor(() => expect(report).toBeDefined());
  act(() => report({ downloaded: 0, total: null, phase: "install" }));
  expect(screen.getByRole("button", { name: "Abbrechen" })).toBeDisabled();
  await app.user.keyboard("{Escape}"); expect(cancel).not.toHaveBeenCalled();
  await act(async () => finish());
  expect(screen.getByRole("alert")).toHaveTextContent("Restart failed");
  await app.user.click(screen.getByRole("button", { name: "Erneut starten" }));
  expect(restart).toHaveBeenCalledTimes(2);
});

test("English groups and banner use English controls", async () => {
  const app = await setupApp({ language: "en", wrap: api => ({ ...api, updateCheck: async () => available }) });
  expect(await screen.findByRole("button", { name: "Open release page" })).toBeInTheDocument();
  act(() => app.runtime.controller.openDialog({ kind: "settings" }));
  expect(screen.getByRole("region", { name: "Updates" })).toHaveTextContent("Installed version");
  expect(screen.getByRole("checkbox", { name: /Check for updates on startup/ })).toBeChecked();
});

test("startup is nonblocking and only checks once even when settings reopen", async () => {
  let done!: (value: null) => void;
  const check = vi.fn(() => new Promise<null>(resolve => { done = resolve; }));
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: check }) });
  expect(screen.getAllByRole("grid")).toHaveLength(2);
  const settings = await settingsDialog(app);
  expect(settings.getByRole("button", { name: "Jetzt prüfen" })).toBeDisabled();
  await app.user.click(settings.getByRole("button", { name: "Fertig" }));
  await settingsDialog(app);
  expect(check).toHaveBeenCalledOnce();
  await act(async () => done(null));
});

test("failed settings persistence prevents native installation", async () => {
  const install = vi.fn(async () => {});
  const app = await setupApp({ settings: { checkUpdatesOnStart: false }, wrap: api => ({ ...api,
    systemStatus: async () => ({ appImage: true, startMenuExists: true }),
    updateCheck: async () => available, updateInstall: install,
    saveSettings: async () => { throw new Error("Disk full"); },
  }) });
  await act(async () => app.runtime.system.check(true));
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  await waitFor(() => expect(app.runtime.system.getSnapshot().installing).toBe(false));
  expect(install).not.toHaveBeenCalled();
  expect(screen.getByRole("region", { name: "Update verfügbar" })).toHaveTextContent("Disk full");
});
