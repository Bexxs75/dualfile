import { describe, expect, test } from "vitest";
import { CoreError } from "./api";
import { createMockApi, type MockApi, type MockOptions, type MockScheduler } from "./mock";
import { demoManifestHash, sanitizePluginText } from "./mock-plugins";
import { MAX_PLUGIN_COLUMN_IDS, MAX_PLUGIN_COLUMN_PATHS } from "./types";
import type { JobStatus, PluginInfo } from "./types";

const HOME = "/home/demo";
const ROOT = "plugin:demo-fs!/";

class ManualScheduler implements MockScheduler {
  private timers = new Map<number, () => void>();
  private next = 1;
  setTimeout(callback: () => void): unknown {
    const id = this.next++;
    this.timers.set(id, callback);
    return id;
  }
  clearTimeout(handle: unknown): void {
    this.timers.delete(handle as number);
  }
  async runAll(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 0));
    for (;;) {
      const first = this.timers.entries().next();
      if (first.done) return;
      this.timers.delete(first.value[0]);
      first.value[1]();
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  }
}

function setup(plugins?: MockOptions["plugins"]): {
  api: MockApi;
  scheduler: ManualScheduler;
} {
  const scheduler = new ManualScheduler();
  const api = createMockApi({ scheduler, now: () => Date.UTC(2026, 10, 12, 12, 0), plugins });
  return { api, scheduler };
}

const kindOf = async (promise: Promise<unknown>): Promise<string> => {
  try {
    await promise;
  } catch (error) {
    return error instanceof CoreError ? error.kind : "kein CoreError";
  }
  return "kein Fehler";
};
const jobOf = async (api: MockApi, id: number): Promise<JobStatus> => {
  const job = (await api.jobs()).find((candidate) => candidate.id === id);
  if (!job) throw new Error(`Auftrag ${id} fehlt`);
  return job;
};
const names = async (api: MockApi, path: string): Promise<string[]> => (await api.list(path)).entries.map((entry) => entry.name);
const plugin = async (api: MockApi, id: string): Promise<PluginInfo> => {
  const found = (await api.pluginsList()).find((candidate) => candidate.id === id);
  if (!found) throw new Error(`Plugin ${id} fehlt`);
  return found;
};
const activate = async (api: MockApi, id: string, grantWrite = false): Promise<PluginInfo> =>
  api.pluginActivate(id, (await plugin(api, id)).manifestHash, grantWrite, false, false);

describe("Liste", () => {
  test("alle sechs Zustände, gültige Hashes, Sandbox-Prüfung", async () => {
    const { api } = setup();
    const list = await api.pluginsList();
    expect(Object.fromEntries(list.map((p) => [p.id, p.status]))).toEqual({
      "demo-fs": "enabled",
      "demo-new": "new",
      "demo-off": "disabled",
      "demo-locked": "locked",
      "demo-nosandbox": "sandboxUnavailable",
      "demo-failed": "failed",
    });
    for (const p of list) {
      expect(p.manifestHash).toMatch(/^[0-9a-f]{64}$/);
      expect(p.enabled).toBe(p.status === "enabled");
      expect(p.sandbox.available).toBe(p.id !== "demo-nosandbox");
    }
    expect(new Set(list.map((p) => p.manifestHash)).size).toBe(list.length);
    const byId = Object.fromEntries(list.map((p) => [p.id, p]));
    expect(byId["demo-fs"]).toMatchObject({
      kinds: ["filesystem", "column", "preview"],
      wantsWrite: true,
      writeGranted: false,
      filesystemRoot: ROOT,
      columns: [
        { id: "demo-fs:laenge", title: "Länge" },
        { id: "demo-fs:gross", title: "Gross" },
      ],
    });
    expect(byId["demo-new"]).toMatchObject({ kinds: ["packer"], packerExtensions: ["dfz"], filesystemRoot: null });
    expect(byId["demo-locked"]).toMatchObject({ problem: "Das Plugin wurde verändert", disabledReason: "locked" });
    expect(byId["demo-failed"]).toMatchObject({ failures: 3, disabledReason: "failures" });
    expect(byId["demo-failed"]!.diagnostic.length).toBeGreaterThan(0);
    expect(demoManifestHash("demo-fs")).toBe(byId["demo-fs"]!.manifestHash);
  });

  test("Rückgabewerte sind Kopien; der Zustand gehört zur Mock-Instanz", async () => {
    const a = setup().api;
    const b = setup().api;
    (await a.pluginsList())[0]!.name = "verändert";
    expect((await a.pluginsList())[0]!.name).not.toBe("verändert");
    await a.pluginDeactivate("demo-fs");
    expect((await plugin(b, "demo-fs")).enabled).toBe(true);
  });
});

describe("Aktivieren", () => {
  test("falscher Hash: pluginLocked, Zustand unverändert", async () => {
    const { api } = setup();
    expect(await kindOf(api.pluginActivate("demo-new", "0".repeat(64), false, false, false))).toBe("pluginLocked");
    expect(await kindOf(api.pluginActivate("demo-new", "", false, false, false))).toBe("pluginLocked");
    expect((await plugin(api, "demo-new")).status).toBe("new");
  });

  test("gesperrtes Plugin bleibt auch mit passendem Hash gesperrt; erst trustReset, dann new und aktivierbar", async () => {
    const { api } = setup();
    expect(await kindOf(activate(api, "demo-locked"))).toBe("pluginLocked");
    expect((await plugin(api, "demo-locked")).status).toBe("locked");
    await api.pluginTrustReset("demo-locked");
    expect(await plugin(api, "demo-locked")).toMatchObject({ status: "new", disabledReason: null, problem: null });
    expect(await activate(api, "demo-locked")).toMatchObject({ status: "enabled", enabled: true });
  });

  test("unsandboxed ohne Bestätigung: invalidInput", async () => {
    const { api } = setup();
    const hash = (await plugin(api, "demo-new")).manifestHash;
    expect(await kindOf(api.pluginActivate("demo-new", hash, false, true, false))).toBe("invalidInput");
    expect((await plugin(api, "demo-new")).status).toBe("new");
    expect(await api.pluginActivate("demo-new", hash, false, true, true)).toMatchObject({ status: "enabled", unsandboxed: true });
  });

  test("ohne Sandbox: pluginSandboxUnavailable, mit Bestätigung geht es", async () => {
    const { api } = setup();
    const hash = (await plugin(api, "demo-nosandbox")).manifestHash;
    expect(await kindOf(api.pluginActivate("demo-nosandbox", hash, false, false, false))).toBe("pluginSandboxUnavailable");
    expect(await kindOf(api.pluginActivate("demo-nosandbox", hash, false, true, false))).toBe("invalidInput");
    expect(await api.pluginActivate("demo-nosandbox", hash, false, true, true)).toMatchObject({
      status: "enabled",
      unsandboxed: true,
      enabled: true,
    });
  });

  test("unbekannte Id: notFound; grantWrite nur bei wantsWrite; Aktivieren setzt failures zurück", async () => {
    const { api } = setup();
    expect(await kindOf(api.pluginActivate("gibt-es-nicht", "0".repeat(64), false, false, false))).toBe("notFound");
    expect(await kindOf(api.pluginDeactivate("gibt-es-nicht"))).toBe("notFound");
    expect(await kindOf(api.pluginTrustReset("gibt-es-nicht"))).toBe("notFound");
    expect(await kindOf(api.pluginDiagnostics("gibt-es-nicht"))).toBe("notFound");
    expect(await activate(api, "demo-new", true)).toMatchObject({ writeGranted: false, wantsWrite: false });
    expect(await activate(api, "demo-fs", true)).toMatchObject({ writeGranted: true });
    expect(await activate(api, "demo-failed")).toMatchObject({ status: "enabled", failures: 0, disabledReason: null, problem: null });
  });

  test("Deaktivieren: disabled; trustReset: new, Rechte weg, deaktiviert", async () => {
    const { api } = setup();
    await activate(api, "demo-fs", true);
    expect(await api.pluginDeactivate("demo-fs")).toMatchObject({ status: "disabled", enabled: false, filesystemRoot: null });
    await activate(api, "demo-fs", true);
    await api.pluginTrustReset("demo-fs");
    expect(await plugin(api, "demo-fs")).toMatchObject({ status: "new", enabled: false, writeGranted: false, unsandboxed: false });
  });
});

describe("Demo-Dateisystem", () => {
  test("aktiv: Listing, Info, Lesen, Suchen, Kopieren nach lokal", async () => {
    const { api, scheduler } = setup();
    expect(await names(api, ROOT)).toEqual(["liesmich.txt", "bild.png", "dokumente", "leer"]);
    expect(await names(api, `${ROOT}dokumente`)).toEqual(["notizen.txt", "liste.csv"]);
    expect((await api.list(ROOT)).path).toBe(ROOT);
    expect((await api.fileInfo(`${ROOT}liesmich.txt`)).size).toBe(256);
    expect((await api.readBlock(`${ROOT}liesmich.txt`, 0, 16)).length).toBe(16);
    expect((await api.findInFile(`${ROOT}liesmich.txt`, new TextEncoder().encode("a"), 0, "forward", true)).kind).toMatch(/found|notFound|continue/);
    const id = await api.copy([`${ROOT}dokumente/notizen.txt`], `${HOME}/Musik`);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    expect(await names(api, `${HOME}/Musik`)).toContain("notizen.txt");
    expect(await names(api, `${ROOT}dokumente`)).toContain("notizen.txt"); // Quelle bleibt
  });

  test("nicht aktiv, unbekannt oder gesperrt: disconnected", async () => {
    const { api } = setup();
    for (const path of ["plugin:demo-off!/", "plugin:demo-locked!/", "plugin:demo-new!/", "plugin:gibt-es-nicht!/", "plugin:demo-nosandbox!/x"]) {
      expect([path, await kindOf(api.list(path))]).toEqual([path, "disconnected"]);
    }
    expect(await kindOf(api.fileInfo("plugin:demo-off!/x.txt"))).toBe("disconnected");
    expect(await kindOf(api.copy(["plugin:demo-off!/x.txt"], `${HOME}/Musik`))).toBe("disconnected");
  });

  test("nach dem Deaktivieren: disconnected; nach erneutem Aktivieren wieder da", async () => {
    const { api } = setup();
    await api.pluginDeactivate("demo-fs");
    const calls = {
      list: () => api.list(ROOT),
      info: () => api.fileInfo(`${ROOT}liesmich.txt`),
      block: () => api.readBlock(`${ROOT}liesmich.txt`, 0, 4),
      mkdir: () => api.makeDir(ROOT, "x"),
      trash: () => api.trash([`${ROOT}liesmich.txt`]),
    };
    for (const [name, run] of Object.entries(calls)) {
      expect([name, await kindOf(run())]).toEqual([name, name === "trash" ? "unsupported" : "disconnected"]);
    }
    await activate(api, "demo-fs");
    expect(await names(api, ROOT)).toContain("liesmich.txt");
  });

  test("gefälschte Pfade und „..“: invalidPath", async () => {
    const { api } = setup();
    for (const path of ["plugin:BAD/ID!/x", "plugin:demo-fs!/../x", "plugin:demo-fs!/a/../b", "plugin:demo-fs", "plugin:!/x", "plugin:demo-fs!x"]) {
      expect([path, await kindOf(api.list(path))]).toEqual([path, "invalidPath"]);
    }
  });

  test("Schreiben ohne Erlaubnis: readOnly; Papierkorb: immer unsupported", async () => {
    const { api } = setup();
    const writes = {
      makeDir: () => api.makeDir(ROOT, "neu"),
      rename: () => api.rename(`${ROOT}liesmich.txt`, "x.txt"),
      renameMany: () => api.renameMany(ROOT, [{ oldName: "liesmich.txt", newName: "x.txt" }]),
      planDelete: () => api.planDelete([`${ROOT}liesmich.txt`]),
      copyHin: () => api.copy([`${HOME}/Dokumente/notizen.txt`], ROOT),
      moveHin: () => api.move([`${HOME}/Dokumente/notizen.txt`], ROOT),
      moveWeg: () => api.move([`${ROOT}liesmich.txt`], `${HOME}/Musik`),
      trash: () => api.trash([`${ROOT}liesmich.txt`]),
      restore: () => api.restoreFromTrash([`${ROOT}liesmich.txt`]),
    };
    const homeNames = await names(api, `${HOME}/Dokumente`);
    expect(homeNames.length).toBeGreaterThan(0);
    for (const [name, run] of Object.entries(writes)) {
      const expected = name === "trash" || name === "restore" ? "unsupported" : "readOnly";
      expect([name, await kindOf(run())]).toEqual([name, expected]);
    }
    expect(await names(api, ROOT)).toEqual(["liesmich.txt", "bild.png", "dokumente", "leer"]);
  });

  test("ein Plugin ohne wantsWrite bleibt auch mit grantWrite schreibgeschützt", async () => {
    const { api } = setup([{ id: "ro-fs", name: "Nur lesen", version: "1", kinds: ["filesystem"], status: "disabled" }]);
    await activate(api, "ro-fs", true);
    expect((await plugin(api, "ro-fs")).writeGranted).toBe(false);
    expect(await kindOf(api.makeDir("plugin:ro-fs!/", "x"))).toBe("readOnly");
  });

  test("mit Erlaubnis: Anlegen, Umbenennen, Kopieren hin, Löschen; Papierkorb bleibt unsupported", async () => {
    const { api, scheduler } = setup();
    await activate(api, "demo-fs", true);
    expect(await api.makeDir(ROOT, "neu")).toBe(`${ROOT}neu`);
    expect((await api.rename(`${ROOT}neu`, "neuer")).newPath).toBe(`${ROOT}neuer`);
    const id = await api.copy([`${HOME}/Dokumente/notizen.txt`], `${ROOT}neuer`);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    expect(await names(api, `${ROOT}neuer`)).toEqual(["notizen.txt"]);
    expect(await kindOf(api.trash([`${ROOT}neuer`]))).toBe("unsupported");
    const plan = await api.planDelete([`${ROOT}neuer`]);
    const del = await api.deletePermanent(plan, plan.token);
    await scheduler.runAll();
    expect((await jobOf(api, del)).state).toBe("done");
    expect(await names(api, ROOT)).not.toContain("neuer");
  });

  test("Packen und Abgleichen auf einen Plugin-Ort: unsupported; watch überspringt Plugin-Pfade", async () => {
    const { api } = setup();
    await activate(api, "demo-fs", true);
    expect(await kindOf(api.packArchive([`${HOME}/Dokumente`], ROOT, "a.zip", "zip"))).toBe("unsupported");
    await expect(api.watch([ROOT, HOME])).resolves.toBeUndefined();
  });

  test("planSync mit Plugin-Ziel wird abgelehnt", async () => {
    const { api, scheduler } = setup();
    await activate(api, "demo-fs", true);
    const id = await api.compareDirs(`${HOME}/Dokumente`, ROOT, { mode: "sizeTime", recursive: false });
    await scheduler.runAll();
    expect(await kindOf(api.planSync(id, "leftToRight", { replaceNewer: false }))).toBe("unsupported");
  });
});

describe("Spalten", () => {
  test("laenge und gross, Verzeichnisse null, gleiche Reihenfolge wie angefragt", async () => {
    const { api } = setup();
    const paths = [`${ROOT}liesmich.txt`, `${ROOT}dokumente`, `${ROOT}bild.png`];
    const result = await api.pluginColumns(paths, ["demo-fs:gross", "demo-fs:laenge"]);
    expect(result).toEqual([
      { columnId: "demo-fs:gross", values: ["LIESMICH.TXT", null, "BILD.PNG"] },
      { columnId: "demo-fs:laenge", values: ["12", null, "8"] },
    ]);
    expect(await api.pluginColumns([], [])).toEqual([]);
  });

  test("Stapelgrenzen: 500 Pfade, 8 Spalten-Ids; unbekannte und inaktive Spalten: notFound", async () => {
    const { api } = setup();
    const path = `${ROOT}liesmich.txt`;
    const ok = await api.pluginColumns(Array(MAX_PLUGIN_COLUMN_PATHS).fill(path), ["demo-fs:laenge"]);
    expect(ok[0]!.values).toHaveLength(MAX_PLUGIN_COLUMN_PATHS);
    expect(await kindOf(api.pluginColumns(Array(MAX_PLUGIN_COLUMN_PATHS + 1).fill(path), ["demo-fs:laenge"]))).toBe("invalidInput");
    expect((await api.pluginColumns([path], Array(MAX_PLUGIN_COLUMN_IDS).fill("demo-fs:laenge"))).length).toBe(MAX_PLUGIN_COLUMN_IDS);
    expect(await kindOf(api.pluginColumns([path], Array(MAX_PLUGIN_COLUMN_IDS + 1).fill("demo-fs:laenge")))).toBe("invalidInput");
    expect(await kindOf(api.pluginColumns([path], ["demo-fs:gibt-es-nicht"]))).toBe("notFound");
    expect(await kindOf(api.pluginColumns([path], ["unbekannt:x"]))).toBe("notFound");
    await api.pluginDeactivate("demo-fs");
    expect(await kindOf(api.pluginColumns([`${HOME}/Dokumente/notizen.txt`], ["demo-fs:laenge"]))).toBe("notFound");
  });

  test("Zellen: Werte lokaler Dateien, fehlende Pfade null, schlechte Pfade invalidPath", async () => {
    const { api } = setup();
    const result = await api.pluginColumns([`${HOME}/Dokumente/notizen.txt`, `${HOME}/gibt-es-nicht`], ["demo-fs:gross"]);
    expect(result[0]!.values).toEqual(["NOTIZEN.TXT", null]);
    expect(await kindOf(api.pluginColumns(["relativ"], ["demo-fs:gross"]))).toBe("invalidPath");
    expect(await kindOf(api.pluginColumns(["plugin:BAD/ID!/x"], ["demo-fs:gross"]))).toBe("invalidPath");
  });

  test("Ordner mit langem Namen liefert null (Kürzung der Zellen: siehe Bereinigung)", async () => {
    const { api } = setup();
    await activate(api, "demo-fs", true);
    const long = `${"x".repeat(250)}.txt`; // 254 Zeichen, gültiger Name
    await api.makeDir(ROOT, long);
    const [gross, laenge] = await api.pluginColumns([`${ROOT}${long}`, `${ROOT}liesmich.txt`], ["demo-fs:gross", "demo-fs:laenge"]);
    expect(gross!.values).toEqual([null, "LIESMICH.TXT"]); // Ordner → null
    expect(laenge!.values[1]).toBe("12");
  });
});

describe("Vorschau", () => {
  test("Text und PNG", async () => {
    const { api } = setup();
    const text = await api.pluginPreview(`${ROOT}liesmich.txt`, "demo-fs");
    expect(text.kind).toBe("text");
    if (text.kind === "text") expect(text.text.length).toBeGreaterThan(0);
    const image = await api.pluginPreview(`${ROOT}bild.png`, "demo-fs");
    expect(image).toMatchObject({ kind: "image", mime: "image/png" });
    if (image.kind === "image") {
      const bytes = Uint8Array.from(atob(image.data), (c) => c.charCodeAt(0));
      expect(Array.from(bytes.slice(0, 8))).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    }
    expect((await api.pluginPreview(`${ROOT}liesmich.txt`)).kind).toBe("text"); // erstes aktives Vorschau-Plugin
    expect((await api.pluginPreview(`${HOME}/Dokumente/notizen.txt`, "demo-fs")).kind).toBe("text"); // auch lokale Dateien
  });

  test("SVG, HTML, Ordner und unbekannte Endungen: unsupported (nie HTML/SVG); fehlende Datei: notFound", async () => {
    const { api, scheduler } = setup();
    await activate(api, "demo-fs", true);
    for (const name of ["a.svg", "b.html", "c.SVG", "d.bin"]) {
      const id = await api.copy([`${HOME}/Dokumente/notizen.txt`], ROOT);
      await scheduler.runAll();
      expect((await jobOf(api, id)).state).toBe("done");
      await api.rename(`${ROOT}notizen.txt`, name);
      expect([name, await kindOf(api.pluginPreview(`${ROOT}${name}`, "demo-fs"))]).toEqual([name, "unsupported"]);
    }
    expect(await kindOf(api.pluginPreview(`${ROOT}dokumente`, "demo-fs"))).toBe("unsupported");
    expect(await kindOf(api.pluginPreview(`${ROOT}gibt-es-nicht.txt`, "demo-fs"))).toBe("notFound");
  });

  test("Plugin nicht aktiv: pluginDisabled; unbekannt: notFound; ohne Vorschau-Art: unsupported", async () => {
    const { api } = setup();
    expect(await kindOf(api.pluginPreview(`${ROOT}liesmich.txt`, "demo-nosandbox"))).toBe("pluginDisabled");
    expect(await kindOf(api.pluginPreview(`${ROOT}liesmich.txt`, "demo-failed"))).toBe("pluginDisabled");
    expect(await kindOf(api.pluginPreview(`${ROOT}liesmich.txt`, "gibt-es-nicht"))).toBe("notFound");
    await activate(api, "demo-new");
    expect(await kindOf(api.pluginPreview(`${ROOT}liesmich.txt`, "demo-new"))).toBe("unsupported");
    await api.pluginDeactivate("demo-fs");
    expect(await kindOf(api.pluginPreview(`${HOME}/Dokumente/notizen.txt`))).toBe("unsupported");
  });
});

describe("Bereinigung", () => {
  const dirty = "A\u0000B\u0007C\u001bD\u007fE\u0085F";
  const extra = [
    {
      id: "dirty",
      name: `${dirty}${"n".repeat(100)}`,
      version: `1${"0".repeat(100)}\u0001`,
      kinds: ["column" as const],
      status: "enabled" as const,
      problem: `${dirty}${"p".repeat(400)}`,
      diagnostic: `Zeile 1\n${dirty}${"d".repeat(1000)}`,
      columns: [{ id: "dirty:t", title: `${dirty}${"t".repeat(100)}` }],
    },
  ];

  test("Name 64, Titel 40, Diagnose 512 Zeichen; Steuerzeichen werden zu U+FFFD", async () => {
    const { api } = setup(extra);
    const p = await plugin(api, "dirty");
    expect(p.name).toHaveLength(64);
    expect(p.name.startsWith("A�B�C�D�E�F")).toBe(true);
    expect(p.columns[0]!.title).toHaveLength(40);
    expect(p.columns[0]!.title).not.toMatch(/[\u0000-\u001f\u007f-\u009f]/);
    expect(p.diagnostic).toHaveLength(512);
    expect(p.diagnostic).not.toMatch(/[\u0000-\u001f\u007f-\u009f]/); // auch kein Zeilenumbruch
    expect(p.version.length).toBeLessThanOrEqual(32);
    expect(p.problem!.length).toBeLessThanOrEqual(200);
    expect(p.problem).not.toMatch(/[\u0000-\u001f\u007f-\u009f]/);
    expect(await api.pluginDiagnostics("dirty")).toBe(p.diagnostic);
    for (const text of [p.name, p.diagnostic, p.columns[0]!.title]) expect(text).not.toContain("\u0000");
  });

  test("Vorschautext behält \\n und \\t, sonst gilt dasselbe", () => {
    expect(sanitizePluginText("a\nb\tc\u0000d\r", 100, true)).toBe("a\nb\tc�d�");
    expect(sanitizePluginText("a\nb", 100)).toBe("a�b");
    expect(sanitizePluginText("😀😀😀", 3)).toBe("😀"); // kein zerrissenes Ersatzpaar
    expect(sanitizePluginText("x".repeat(300_000), 256 * 1024, true)).toHaveLength(256 * 1024);
  });
});
