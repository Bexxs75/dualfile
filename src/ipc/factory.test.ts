import { beforeEach, describe, expect, test, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));

import { createApi } from "./factory";

beforeEach(() => invokeMock.mockReset());

describe("Auswahl der API", () => {
  test("mit __TAURI_INTERNALS__ trägt der Kern (Tauri-Befehle)", async () => {
    invokeMock.mockResolvedValue("/home/echt");
    const api = createApi({ __TAURI_INTERNALS__: {} });
    expect(await api.homeDir()).toBe("/home/echt");
    expect(invokeMock).toHaveBeenCalledWith("home_dir", undefined);
  });

  test("ohne __TAURI_INTERNALS__ (Browser-Entwurf) läuft die Mock-API", async () => {
    const api = createApi({});
    expect(await api.homeDir()).toBe("/home/demo");
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

test("browser discovery preview has two opt-in NAS hosts and opens a prefilled editor", async () => {
  const api = createApi({});
  const settings = (await api.loadSettings()).settings;
  expect((await api.discoverySnapshot())).toEqual([]);
  await api.saveSettings({ ...settings, sidebarDiscovery: true });
  const services = (await api.discoverySnapshot())!;
  expect(services).toHaveLength(2);
  const open = vi.spyOn(window, "open").mockReturnValue({ closed: false, focus: vi.fn() } as unknown as Window);
  try {
    expect(await api.connectionEditorOpen(null, "Editor", { theme: "papier", accent: "#112233", accentAutomatic: false, density: "normal", fontSize: 13, fontWeight: "normal", lang: "en" }, services[0]!.key)).toBe(true);
    const q = new URL(String(open.mock.calls[0]![0]), window.location.href).searchParams;
    expect(q.get("view")).toBe("connection-editor");
    expect(JSON.parse(q.get("discovered")!)).toEqual(services[0]);
    expect(await api.connectionsList()).toEqual([]);
    await api.saveSettings(settings);
    expect((await api.discoverySnapshot())).toEqual([]);
  } finally { open.mockRestore(); }
});
