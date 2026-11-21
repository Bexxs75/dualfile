import { describe, expect, test, vi } from "vitest";
import { createSystemController, isNewerVersion } from "./system";
import { defaultSettings } from "./state";
import type { AutostartMode, SystemApi } from "../ipc/system";

function fixture(appImage = true) {
  let settings = { ...defaultSettings };
  let busy = false;
  const api: SystemApi = {
    systemStatus: vi.fn(async () => ({ appImage, startMenuExists: false, autostart: "off" as const })),
    startMenuSet: vi.fn(async () => {}), traySet: vi.fn(async () => true),
    autostartSet: vi.fn(async (mode: AutostartMode) => mode),
    updateCheck: vi.fn(async () => ({ version: "1.2.0", notes: "Release notes" })),
    updateInstall: vi.fn(async (_version, progress) => { progress({ downloaded: 12, total: 12, phase: "download" }); }),
    updateCancel: vi.fn(async () => true),
    restart: vi.fn(async () => {}), releaseOpen: vi.fn(async () => {}),
  };
  const save = vi.fn(async () => {});
  const make = () => createSystemController(api, () => settings, patch => { settings = { ...settings, ...patch }; }, () => busy, save, () => "2026-01-01T00:00:00Z");
  return { api, save, make, controller: make(), settings: () => settings,
    patch: (patch: Partial<typeof settings>) => { settings = { ...settings, ...patch }; }, busy: () => { busy = true; } };
}

describe("UPD1 policy", () => {
  test("one nonblocking startup check, never installs automatically", async () => {
    const f = fixture();
    await f.controller.start("de"); await f.controller.start("de");
    await vi.waitFor(() => expect(f.controller.getSnapshot().check).toBe("available"));
    expect(f.api.updateCheck).toHaveBeenCalledTimes(1);
    expect(f.api.updateInstall).not.toHaveBeenCalled();
    expect(f.settings().lastUpdateCheck).toBe("2026-01-01T00:00:00Z");
  });
  test("startup preference, manual current result and error", async () => {
    const f = fixture(); f.patch({ checkUpdatesOnStart: false });
    await f.controller.start("en"); expect(f.api.updateCheck).not.toHaveBeenCalled();
    vi.mocked(f.api.updateCheck).mockResolvedValueOnce(null);
    await f.controller.check(true); expect(f.controller.getSnapshot().check).toBe("current");
    vi.mocked(f.api.updateCheck).mockRejectedValueOnce(new Error("Offline"));
    const log = vi.spyOn(console, "warn").mockImplementation(() => {});
    await f.controller.check();
    expect(f.controller.getSnapshot().error).toBe("Offline");
    expect(log).toHaveBeenCalledOnce(); log.mockRestore();
  });
  test("Later persists version; only newer banners return; manual check can redisplay", async () => {
    const f = fixture(); await f.controller.check(); f.controller.dismiss();
    expect(f.settings().dismissedUpdateVersion).toBe("1.2.0");
    await f.controller.check(); expect(f.controller.getSnapshot().update).toBeNull();
    vi.mocked(f.api.updateCheck).mockResolvedValueOnce({ version: "1.1.0", notes: "" });
    await f.controller.check(); expect(f.controller.getSnapshot().update).toBeNull();
    vi.mocked(f.api.updateCheck).mockResolvedValueOnce({ version: "1.3.0", notes: "new" });
    await f.controller.check(); expect(f.controller.getSnapshot().update?.version).toBe("1.3.0");
    f.controller.dismiss(); await f.controller.check(true);
    expect(f.controller.getSnapshot().update?.version).toBe("1.2.0");
  });
  test("active file jobs prohibit installation and release opening", async () => {
    for (const appImage of [true, false]) {
      const f = fixture(appImage); await f.controller.start("en"); await f.controller.check();
      f.busy(); await f.controller.install();
      expect(f.api.updateInstall).not.toHaveBeenCalled(); expect(f.api.releaseOpen).not.toHaveBeenCalled();
    }
  });
  test("without APPIMAGE only opens release link; AppImage saves, installs, restarts", async () => {
    const plain = fixture(false); await plain.controller.start("en"); await plain.controller.check();
    await plain.controller.install(); expect(plain.api.releaseOpen).toHaveBeenCalledOnce();
    expect(plain.api.updateInstall).not.toHaveBeenCalled(); expect(plain.api.restart).not.toHaveBeenCalled();
    const image = fixture(); await image.controller.start("en"); await image.controller.check();
    await image.controller.install();
    expect(image.api.updateInstall).toHaveBeenCalledWith("1.2.0", expect.any(Function));
    expect(image.api.restart).toHaveBeenCalledOnce(); expect(image.controller.getSnapshot().progress?.downloaded).toBe(12);
    expect(image.save.mock.invocationCallOrder.at(-1)).toBeLessThan(vi.mocked(image.api.updateInstall).mock.invocationCallOrder[0]!);
  });
  test("failed signature/install never restarts; retry remains possible", async () => {
    const f = fixture(); await f.controller.start("de"); await f.controller.check();
    vi.mocked(f.api.updateInstall).mockRejectedValueOnce(new Error("Invalid signature"));
    await f.controller.install(); expect(f.api.restart).not.toHaveBeenCalled();
    expect(f.controller.getSnapshot().installing).toBe(false);
    expect(f.controller.getSnapshot().error).toBe("Invalid signature");
    await f.controller.install(); expect(f.api.restart).toHaveBeenCalledOnce();
  });
  test("first-start counter caps prompts at three launches", async () => {
    const f = fixture(); f.patch({ checkUpdatesOnStart: false });
    for (let count = 1; count <= 3; count++) {
      const controller = f.make(); await controller.start("de");
      expect(controller.getSnapshot().prompt).toBe(true); expect(f.settings().startMenuPromptCount).toBe(count);
      await controller.answerPrompt(false, false);
      expect(f.settings().startMenuPrompted).toBe(count === 3);
    }
    const last = f.make(); await last.start("de"); expect(last.getSnapshot().prompt).toBe(false);
  });
  test("checkbox suppresses future prompts, install failure keeps dialog open", async () => {
    const f = fixture(); await f.controller.start("de");
    vi.mocked(f.api.startMenuSet).mockRejectedValueOnce(new Error("Permission denied"));
    await expect(f.controller.answerPrompt(true, false)).rejects.toThrow("Permission denied");
    expect(f.controller.getSnapshot().prompt).toBe(true); expect(f.settings().startMenuPrompted).toBe(false);
    await f.controller.answerPrompt(false, true);
    const next = f.make(); await next.start("de"); expect(next.getSnapshot().prompt).toBe(false);
  });
  test("successful registration marks prompt complete; existing entry never prompts", async () => {
    const f = fixture(); await f.controller.start("de"); await f.controller.answerPrompt(true, false);
    expect(f.settings().startMenuPrompted).toBe(true); expect(f.controller.getSnapshot().status?.startMenuExists).toBe(true);
    const existing = fixture(); vi.mocked(existing.api.systemStatus).mockResolvedValue({ appImage: true, startMenuExists: true, autostart: "off" });
    await existing.controller.start("en"); expect(existing.controller.getSnapshot().prompt).toBe(false);
    expect(existing.settings().startMenuPromptCount).toBe(0);
  });
  test("tray failure records one persistent hint and changes apply in order", async () => {
    const f = fixture(); vi.mocked(f.api.traySet).mockResolvedValue(false);
    await f.controller.setTray(true, "de"); await f.controller.setTray(false, "en");
    expect(f.controller.getSnapshot().trayUnavailable).toBe(true);
    expect(f.api.traySet).toHaveBeenNthCalledWith(1, true, "de");
    expect(f.api.traySet).toHaveBeenNthCalledWith(2, false, "en");
  });
  test.each([
    ["1.10.0", "1.9.0", true], ["1.0.0", "1.0.0-rc.2", true],
    ["1.0.0-rc.10", "1.0.0-rc.2", true], ["1.0.0+new", "1.0.0+old", false],
    ["1.0.0-alpha", "1.0.0", false], ["1.0.0-alpha.1", "1.0.0-alpha", true],
  ])("SemVer %s > %s = %s", (a, b, expected) => expect(isNewerVersion(a, b)).toBe(expected));
});
test("installation waits for the settings-window persistence barrier", async () => {
  const f = fixture();
  let release!: () => void;
  const barrier = vi.fn(() => new Promise<void>(resolve => { release = resolve; }));
  const controller = createSystemController(f.api, () => ({ ...defaultSettings, startMenuPrompted: true }), () => {}, () => false, f.save, undefined, barrier);
  await controller.start("en"); await controller.check(true);
  const install = controller.install();
  await vi.waitFor(() => expect(barrier).toHaveBeenCalledOnce());
  expect(f.api.updateInstall).not.toHaveBeenCalled();
  release(); await install;
  expect(f.api.updateInstall).toHaveBeenCalledOnce();
  expect(f.api.restart).toHaveBeenCalledOnce();
});
test.each(["jobs", "cancel"])("install skips the settings drain when %s occurs during save", async reason => {
  const f = fixture(); let busy = false;
  const barrier = vi.fn(async () => {});
  const save = vi.fn(async () => { if (reason === "jobs") busy = true; else await controller.cancel(); });
  const controller = createSystemController(f.api, () => ({ ...defaultSettings, startMenuPrompted: true }), () => {}, () => busy, save, undefined, barrier);
  await controller.start("en"); await controller.check(true); await controller.install();
  expect(barrier).not.toHaveBeenCalled(); expect(f.api.updateInstall).not.toHaveBeenCalled();
});
test("a failed settings drain cancels installation with a visible system error", async () => {
  const f = fixture();
  const controller = createSystemController(f.api, () => ({ ...defaultSettings, startMenuPrompted: true }), () => {}, () => false, f.save, undefined, async () => { throw new Error("Settings could not close"); });
  await controller.start("en"); await controller.check(true); await controller.install();
  expect(f.api.updateInstall).not.toHaveBeenCalled();
  expect(controller.getSnapshot()).toMatchObject({ installing: false, error: "Settings could not close" });
});

describe("autostart", () => {
  test("asks once while never answered; not now ends with off and does not return", async () => {
    const f = fixture(); await f.controller.start("de");
    expect(f.controller.getSnapshot().autostartPrompt).toBe(true);
    expect(f.settings().autostartPrompted).toBe(false);
    await f.controller.answerAutostart("off");
    expect(f.api.autostartSet).toHaveBeenCalledExactlyOnceWith("off");
    expect(f.settings().autostartPrompted).toBe(true); expect(f.save).toHaveBeenCalled();
    expect(f.controller.getSnapshot().autostartPrompt).toBe(false);
    const next = f.make(); await next.start("de"); expect(next.getSnapshot().autostartPrompt).toBe(false);
  });
  test.each(["minimized", "shown"] as const)("answer %s writes the entry and shows what is on disk", async mode => {
    const f = fixture(); await f.controller.start("de");
    await f.controller.answerAutostart(mode);
    expect(f.api.autostartSet).toHaveBeenCalledWith(mode);
    expect(f.controller.getSnapshot()).toMatchObject({ autostartPrompt: false, status: { autostart: mode } });
    expect(f.settings().autostartPrompted).toBe(true);
  });
  test("a failed write keeps the question open, the setting unanswered and the status unchanged", async () => {
    const f = fixture(); vi.mocked(f.api.autostartSet).mockRejectedValueOnce(new Error("Permission denied"));
    await f.controller.start("de");
    await expect(f.controller.answerAutostart("minimized")).rejects.toThrow("Permission denied");
    expect(f.controller.getSnapshot()).toMatchObject({ autostartPrompt: true, status: { autostart: "off" } });
    expect(f.settings().autostartPrompted).toBe(false);
    vi.mocked(f.api.autostartSet).mockRejectedValueOnce(new Error("Read-only file system"));
    await expect(f.controller.setAutostart("shown")).rejects.toThrow("Read-only file system");
    expect(f.controller.getSnapshot().status?.autostart).toBe("off");
  });
  test("an existing entry counts as answered; settings changes follow the disk, not the request", async () => {
    const f = fixture(); vi.mocked(f.api.systemStatus).mockResolvedValue({ appImage: true, startMenuExists: true, autostart: "minimized", autostartError: "stale" });
    await f.controller.start("en");
    expect(f.controller.getSnapshot().autostartPrompt).toBe(false); expect(f.settings().autostartPrompted).toBe(true);
    vi.mocked(f.api.autostartSet).mockResolvedValueOnce("off");
    await f.controller.setAutostart("shown");
    expect(f.controller.getSnapshot().status).toMatchObject({ autostart: "off", autostartError: null });
    const asked = fixture(); asked.patch({ autostartPrompted: true }); await asked.controller.start("en");
    expect(asked.controller.getSnapshot().autostartPrompt).toBe(false);
    await asked.controller.setAutostart("off");
    expect(asked.api.autostartSet).toHaveBeenCalledWith("off");
  });
});

describe("autostart status follows the disk", () => {
  const status = (autostart: AutostartMode, extra: object = {}) => ({ appImage: true, startMenuExists: true, autostart, ...extra });
  test("refresh re-reads the entries; a failed change reconciles with the disk", async () => {
    const f = fixture(); f.patch({ autostartPrompted: true });
    vi.mocked(f.api.systemStatus).mockResolvedValue(status("minimized"));
    await f.controller.start("en");
    vi.mocked(f.api.systemStatus).mockResolvedValue(status("off"));
    await f.controller.refreshStatus();
    expect(f.controller.getSnapshot().status?.autostart).toBe("off");
    // The write failed, but something else changed the entry meanwhile: show what is there.
    vi.mocked(f.api.autostartSet).mockRejectedValueOnce(new Error("Permission denied"));
    vi.mocked(f.api.systemStatus).mockResolvedValue(status("shown"));
    await expect(f.controller.setAutostart("minimized")).rejects.toThrow("Permission denied");
    expect(f.controller.getSnapshot().status?.autostart).toBe("shown");
  });
  test("a slow older refresh cannot overwrite a newer choice", async () => {
    const f = fixture(); f.patch({ autostartPrompted: true });
    vi.mocked(f.api.systemStatus).mockResolvedValue(status("minimized"));
    await f.controller.start("en");
    let release!: () => void;
    vi.mocked(f.api.systemStatus).mockImplementationOnce(() => new Promise(resolve => { release = () => resolve(status("minimized")); }));
    const refresh = f.controller.refreshStatus();
    const choice = f.controller.setAutostart("off");
    await Promise.resolve(); await Promise.resolve();
    expect(f.api.autostartSet).not.toHaveBeenCalled();
    release(); await refresh; await choice;
    expect(f.api.autostartSet).toHaveBeenCalledExactlyOnceWith("off");
    expect(f.controller.getSnapshot().status?.autostart).toBe("off");
  });
  test("a start-menu error neither asks about the start menu nor blocks autostart", async () => {
    const f = fixture();
    vi.mocked(f.api.systemStatus).mockResolvedValue({ appImage: true, startMenuExists: false, startMenuError: "Symlink in desktop integration path", autostart: "off" });
    await f.controller.start("en");
    expect(f.controller.getSnapshot()).toMatchObject({ prompt: false, autostartPrompt: true });
    await f.controller.answerAutostart("shown");
    expect(f.controller.getSnapshot().status).toMatchObject({ autostart: "shown", startMenuError: "Symlink in desktop integration path" });
  });
  test("not now always ends with off, also when an entry is already enabled", async () => {
    const f = fixture(); await f.controller.start("de");
    // The entry got enabled while the question was still open (earlier answer, other window).
    vi.mocked(f.api.systemStatus).mockResolvedValue({ appImage: true, startMenuExists: false, autostart: "minimized" });
    await f.controller.refreshStatus();
    await f.controller.answerAutostart("off");
    expect(f.api.autostartSet).toHaveBeenCalledExactlyOnceWith("off");
    expect(f.controller.getSnapshot()).toMatchObject({ autostartPrompt: false, status: { autostart: "off" } });
    expect(f.settings().autostartPrompted).toBe(true);
    // Removing it fails: the question stays, nothing is claimed.
    const g = fixture(); await g.controller.start("de");
    vi.mocked(g.api.systemStatus).mockResolvedValue({ appImage: true, startMenuExists: false, autostart: "shown" });
    await g.controller.refreshStatus();
    vi.mocked(g.api.autostartSet).mockRejectedValueOnce(new Error("Read-only file system"));
    await expect(g.controller.answerAutostart("off")).rejects.toThrow("Read-only file system");
    expect(g.controller.getSnapshot()).toMatchObject({ autostartPrompt: true, status: { autostart: "shown" } });
    expect(g.settings().autostartPrompted).toBe(false);
  });
  test("an answer that cannot be saved closes the question, keeps the entry as chosen and reports the error", async () => {
    const f = fixture(); await f.controller.start("de");
    f.save.mockClear(); f.save.mockRejectedValueOnce(new Error("disk full"));
    await f.controller.answerAutostart("minimized");
    expect(f.controller.getSnapshot()).toMatchObject({ autostartPrompt: false, error: "disk full", status: { autostart: "minimized" } });
    expect(f.settings().autostartPrompted).toBe(true);
    const g = fixture(); await g.controller.start("de");
    g.save.mockClear(); g.save.mockRejectedValueOnce(new Error("disk full"));
    await g.controller.answerAutostart("off");
    expect(g.api.autostartSet).toHaveBeenCalledExactlyOnceWith("off");
    expect(g.controller.getSnapshot()).toMatchObject({ autostartPrompt: false, error: "disk full", status: { autostart: "off" } });
  });
  test("not now removes an entry that was enabled outside Dualfile without any refresh in between", async () => {
    const f = fixture(); await f.controller.start("de");
    // Cached status still says off; on disk the entry is enabled meanwhile.
    let disk: AutostartMode = "minimized";
    vi.mocked(f.api.autostartSet).mockImplementation(async mode => disk = mode);
    expect(f.controller.getSnapshot().status?.autostart).toBe("off");
    await f.controller.answerAutostart("off");
    expect(f.api.autostartSet).toHaveBeenCalledExactlyOnceWith("off");
    expect(disk).toBe("off");
    expect(f.controller.getSnapshot()).toMatchObject({ autostartPrompt: false, status: { autostart: "off" } });
  });
});

 test("known Flatpak never checks/installs updates or touches the AppImage starter", async () => {
  const f = fixture();
  vi.mocked(f.api.systemStatus).mockResolvedValue({ packaging: "flatpak", appImage: false, startMenuExists: false, autostart: "off" });
  await f.controller.refreshStatus();
  await f.controller.start("de");
  await f.controller.check(true);
  await f.controller.setStartMenu(true);
  await f.controller.answerPrompt(true, false);
  await f.controller.install();
  expect(f.api.updateCheck).not.toHaveBeenCalled();
  expect(f.api.updateInstall).not.toHaveBeenCalled();
  expect(f.api.startMenuSet).not.toHaveBeenCalled();
  expect(f.controller.getSnapshot().prompt).toBe(false);
  expect(f.settings().startMenuPromptCount).toBe(0);
 });

 test("failed status loading does not block the startup update check", async () => {
  const f = fixture();
  vi.mocked(f.api.systemStatus).mockRejectedValue(new Error("Status unavailable"));
  await f.controller.start("en");
  expect(f.api.updateCheck).toHaveBeenCalledOnce();
  expect(f.controller.getSnapshot()).toMatchObject({ status: null, check: "available" });
 });
 test("pending status loading does not delay the startup update check", async () => {
  const f = fixture();
  vi.mocked(f.api.systemStatus).mockImplementation(() => new Promise(() => {}));
  void f.controller.start("en");
  await vi.waitFor(() => expect(f.api.updateCheck).toHaveBeenCalledOnce());
  expect(f.controller.getSnapshot().status).toBeNull();
 });
