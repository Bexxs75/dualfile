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
