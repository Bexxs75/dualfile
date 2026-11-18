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
  await app.user.click(dlg.getByRole("tab", { name: "System" }));
  expect(dlg.getByRole("region", { name: "System" })).toBeInTheDocument();
  await app.user.click(dlg.getByRole("checkbox", { name: /Symbol im Infobereich/ }));
  expect(tray).toHaveBeenLastCalledWith(true, "de");
  expect(app.runtime.store.getState().settings.trayEnabled).toBe(true);
  expect(dlg.getByText(/Kein Infobereich gefunden/)).toBeInTheDocument();
  await app.user.click(dlg.getByRole("button", { name: "Eintrag entfernen" }));
  expect(menu).toHaveBeenLastCalledWith(false);
  await app.user.click(dlg.getByRole("button", { name: "Eintragen" }));
  expect(menu).toHaveBeenLastCalledWith(true);
  await app.user.click(dlg.getByRole("tab", { name: "Info" }));
  // Reads the version from the same source the dialog does, so a version bump never breaks this test.
  expect(dlg.getByRole("region", { name: "Updates" })).toHaveTextContent(`Installierte Version ${config.version}`);
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
  await app.user.click(settings.getByRole("tab", { name: "Info" }));
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
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "off" }), updateInstall: install }) });
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
  const app = await setupApp({ settings: { checkUpdatesOnStart: false }, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false, autostart: "off" }) }) });
  const dlg = within(await screen.findByRole("dialog", { name: "Dualfile im Startmenü eintragen?" }));
  expect(app.runtime.store.getState().settings.startMenuPromptCount).toBe(1);
  if (never) await app.user.click(dlg.getByRole("checkbox", { name: "Nicht noch einmal fragen" }));
  await app.user.click(dlg.getByRole("button", { name: "Nein, danke" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(app.runtime.store.getState().settings.startMenuPrompted).toBe(never);
  expect((await app.api.loadSettings()).settings.startMenuPrompted).toBe(never);
});

test("third prompt: Escape counts as refusal and no fourth prompt appears", async () => {
  const app = await setupApp({ settings: { startMenuPromptCount: 2 }, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false, autostart: "off" }) }) });
  await screen.findByRole("dialog", { name: "Dualfile im Startmenü eintragen?" });
  await app.user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(app.runtime.store.getState().settings.startMenuPromptCount).toBe(3);
  expect(app.runtime.store.getState().settings.startMenuPrompted).toBe(true);
  app.view.unmount();
  await setupApp({ settings: { startMenuPromptCount: 3, startMenuPrompted: false }, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false, autostart: "off" }) }) });
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("first-start add retries a failed entry and closes after success", async () => {
  const menu = vi.fn(async () => {}).mockRejectedValueOnce(new Error("Read only"));
  const app = await setupApp({ wrap: api => ({ ...api, startMenuSet: menu, systemStatus: async () => ({ appImage: false, startMenuExists: false, autostart: "off" }) }) });
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
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "off" }), restart, updateCancel: cancel,
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

test("install phase blocks Escape/cancel; installed relaunches automatically, failure offers retry via toast", async () => {
  let report!: (p: UpdateProgress) => void, finish!: () => void;
  const restart = vi.fn(async () => {}).mockRejectedValueOnce(new Error("Restart failed"));
  const cancel = vi.fn(async () => true);
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "off" }), restart, updateCancel: cancel,
    updateInstall: async (_v, progress) => { report = progress; await new Promise<void>(resolve => { finish = resolve; }); },
  }) });
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  await waitFor(() => expect(report).toBeDefined());
  act(() => report({ downloaded: 0, total: null, phase: "install" }));
  expect(screen.getByRole("button", { name: "Abbrechen" })).toBeDisabled();
  await app.user.keyboard("{Escape}"); expect(cancel).not.toHaveBeenCalled();
  await act(async () => finish());
  await waitFor(() => expect(restart).toHaveBeenCalledTimes(1));
  expect(await screen.findByRole("alert")).toHaveTextContent("Restart failed");
  await app.user.click(screen.getByRole("button", { name: "Jetzt neu starten" }));
  expect(restart).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("button", { name: "Jetzt neu starten" })).toBeNull();
});

test("English groups and banner use English controls", async () => {
  const app = await setupApp({ language: "en", wrap: api => ({ ...api, updateCheck: async () => available }) });
  expect(await screen.findByRole("button", { name: "Open release page" })).toBeInTheDocument();
  act(() => app.runtime.controller.openDialog({ kind: "settings" }));
  await app.user.click(screen.getByRole("tab", { name: "About" }));
  expect(screen.getByRole("region", { name: "Updates" })).toHaveTextContent("Installed version");
  expect(screen.getByRole("checkbox", { name: /Check for updates on startup/ })).toBeChecked();
});

test("startup is nonblocking and only checks once even when settings reopen", async () => {
  let done!: (value: null) => void;
  const check = vi.fn(() => new Promise<null>(resolve => { done = resolve; }));
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: check }) });
  expect(screen.getAllByRole("grid")).toHaveLength(2);
  const settings = await settingsDialog(app);
  await app.user.click(settings.getByRole("tab", { name: "Info" }));
  expect(settings.getByRole("button", { name: "Jetzt prüfen" })).toBeDisabled();
  await app.user.click(settings.getByRole("button", { name: "Fertig" }));
  await settingsDialog(app);
  expect(check).toHaveBeenCalledOnce();
  await act(async () => done(null));
});

test("failed settings persistence prevents native installation", async () => {
  const install = vi.fn(async () => {});
  const app = await setupApp({ settings: { checkUpdatesOnStart: false }, wrap: api => ({ ...api,
    systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "off" }),
    updateCheck: async () => available, updateInstall: install,
    saveSettings: async () => { throw new Error("Disk full"); },
  }) });
  await act(async () => app.runtime.system.check(true));
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  await waitFor(() => expect(app.runtime.system.getSnapshot().installing).toBe(false));
  expect(install).not.toHaveBeenCalled();
  expect((await screen.findAllByRole("alert"))[0]).toHaveTextContent("Disk full");
  expect(screen.getByRole("region", { name: "Update verfügbar" })).not.toHaveTextContent("Disk full");
});

test("successful install relaunches automatically without a manual click", async () => {
  const restart = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "off" }), restart, updateInstall: async () => {} }) });
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  await waitFor(() => expect(restart).toHaveBeenCalledOnce());
  expect(screen.queryByRole("button", { name: "Jetzt neu starten" })).toBeNull();
});

test("update errors appear as toast and leave the banner unchanged", async () => {
  const app = await setupApp({ wrap: api => ({ ...api, updateCheck: async () => available, systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "off" }), updateInstall: async () => { throw new Error("Download broke"); } }) });
  const banner = screen.getByRole("region", { name: "Update verfügbar" }).textContent;
  await app.user.click(await screen.findByRole("button", { name: "Aktualisieren" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Download broke");
  const region = screen.getByRole("region", { name: "Update verfügbar" });
  expect(region.textContent).toBe(banner);
  expect(region.querySelector("[role=alert]")).toBeNull();
});

test("Info tab shows version, license and a GitHub button that opens the repository", async () => {
  const openUrl = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, openUrl }) });
  const dlg = await settingsDialog(app);
  await app.user.click(dlg.getByRole("tab", { name: "Info" }));
  expect(dlg.getByRole("region", { name: "Über Dualfile" })).toHaveTextContent(`Dualfile ${config.version} · Lizenz: MIT`);
  await app.user.click(dlg.getByRole("button", { name: "Projekt auf GitHub" }));
  expect(openUrl).toHaveBeenCalledWith("https://github.com/Bexxs75/dualfile");
});

const neverAsked = { autostartPrompted: false, checkUpdatesOnStart: false };
test.each(["de", "en"] as const)("autostart question appears once (%s); not now is remembered and ends with off", async language => {
  const set = vi.fn(async (mode: "off" | "minimized" | "shown") => mode);
  const app = await setupApp({ language, settings: neverAsked, wrap: api => ({ ...api, autostartSet: set }) });
  const de = language === "de";
  const dlg = within(await screen.findByRole("dialog", { name: de ? "Dualfile mit dem System starten?" : "Start Dualfile with the system?" }));
  expect(dlg.getByRole("button", { name: de ? "Minimiert starten" : "Start minimized" })).toBeVisible();
  expect(dlg.getByRole("button", { name: de ? "Starten und Fenster zeigen" : "Start and show the window" })).toBeVisible();
  await app.user.click(dlg.getByRole("button", { name: de ? "Nicht jetzt" : "Not now" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(set.mock.calls).toEqual([["off"]]);
  expect((await app.api.loadSettings()).settings.autostartPrompted).toBe(true);
  app.view.unmount();
  await setupApp({ language, settings: { ...neverAsked, autostartPrompted: true } });
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("autostart question: Escape counts as not now; a choice writes the entry", async () => {
  const set = vi.fn(async (mode: "off" | "minimized" | "shown") => mode);
  const first = await setupApp({ settings: neverAsked, wrap: api => ({ ...api, autostartSet: set }) });
  await screen.findByRole("dialog", { name: "Dualfile mit dem System starten?" });
  await first.user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(set.mock.calls).toEqual([["off"]]);
  expect(first.runtime.store.getState().settings.autostartPrompted).toBe(true);
  first.view.unmount();
  const app = await setupApp({ settings: neverAsked, wrap: api => ({ ...api, autostartSet: set }) });
  const dlg = within(await screen.findByRole("dialog", { name: "Dualfile mit dem System starten?" }));
  await app.user.click(dlg.getByRole("button", { name: "Minimiert starten" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(set).toHaveBeenLastCalledWith("minimized");
  expect(app.runtime.system.getSnapshot().status?.autostart).toBe("minimized");
  expect((await app.api.loadSettings()).settings.autostartPrompted).toBe(true);
});

test("autostart question shows a write error, stays open and can still be declined", async () => {
  const set = vi.fn(async (mode: "off" | "minimized" | "shown") => mode).mockRejectedValueOnce(new Error("Permission denied"));
  const app = await setupApp({ settings: neverAsked, wrap: api => ({ ...api, autostartSet: set }) });
  const dlg = within(await screen.findByRole("dialog", { name: "Dualfile mit dem System starten?" }));
  await app.user.click(dlg.getByRole("button", { name: "Starten und Fenster zeigen" }));
  expect(await dlg.findByRole("alert")).toHaveTextContent("Der Autostart-Eintrag konnte nicht geändert werden: Permission denied");
  expect(app.runtime.store.getState().settings.autostartPrompted).toBe(false);
  await app.user.click(dlg.getByRole("button", { name: "Nicht jetzt" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(app.runtime.store.getState().settings.autostartPrompted).toBe(true);
});

test("startup questions come one at a time: what's new, then start menu, then autostart", async () => {
  const app = await setupApp({ settings: { ...neverAsked, lastSeenVersion: "0.3.5" },
    wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: false, autostart: "off" }) }) });
  await waitFor(() => expect(app.runtime.system.getSnapshot()).toMatchObject({ prompt: true, autostartPrompt: true }));
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  expect(app.runtime.store.getState().dialog?.kind).toBe("whatsNew");
  await app.user.keyboard("{Escape}");
  const menu = within(await screen.findByRole("dialog", { name: "Dualfile im Startmenü eintragen?" }));
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  await app.user.click(menu.getByRole("button", { name: "Nein, danke" }));
  await screen.findByRole("dialog", { name: "Dualfile mit dem System starten?" });
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  await app.user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

test("an existing autostart entry is never asked about", async () => {
  const app = await setupApp({ settings: neverAsked, wrap: api => ({ ...api, systemStatus: async () => ({ appImage: true, startMenuExists: true, autostart: "shown" }) }) });
  await waitFor(() => expect(app.runtime.store.getState().settings.autostartPrompted).toBe(true));
  expect(screen.queryByRole("dialog")).toBeNull();
});
