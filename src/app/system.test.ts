import { describe, expect, test, vi } from "vitest";
import { createSystemController, isNewerVersion } from "./system";
import { defaultSettings } from "./state";
import type { SystemApi } from "../ipc/system";

function fixture(appImage = true) {
  let settings = { ...defaultSettings };
  let busy = false;
  const api: SystemApi = {
    systemStatus: vi.fn(async () => ({ appImage, startMenuExists: false })),
    startMenuSet: vi.fn(async () => {}), traySet: vi.fn(async () => true),
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
    const existing = fixture(); vi.mocked(existing.api.systemStatus).mockResolvedValue({ appImage: true, startMenuExists: true });
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
