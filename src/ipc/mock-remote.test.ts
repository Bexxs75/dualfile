import { describe, expect, test } from "vitest";
import { CoreError, type DualfileApi } from "./api";
import { createMockApi, DEMO_FINGERPRINT_FTPS, DEMO_FINGERPRINT_SFTP, MAX_PROFILES, type MockApi, type MockScheduler } from "./mock";
import type { ConnectOutcome, ConnectionProfile, CoreEvent, JobStatus } from "./types";

const HOME = "/home/demo";
const MARKER = "geheim-marker-4711";

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
  async settle(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  async step(): Promise<boolean> {
    const first = this.timers.entries().next();
    if (first.done) return false;
    this.timers.delete(first.value[0]);
    first.value[1]();
    await this.settle();
    return true;
  }
  async runAll(): Promise<void> {
    await this.settle();
    while (await this.step()) {
      /* weiter */
    }
  }
}

function setup(options: { keychain?: boolean } = {}): { api: MockApi; scheduler: ManualScheduler; events: CoreEvent[] } {
  const scheduler = new ManualScheduler();
  const api = createMockApi({ scheduler, now: () => Date.UTC(2026, 10, 12, 12, 0), ...options });
  const events: CoreEvent[] = [];
  api.onEvent((event) => events.push(event));
  return { api, scheduler, events };
}

const profile = (over: Partial<ConnectionProfile> = {}): ConnectionProfile => ({
  id: "",
  name: "Demo SFTP",
  protocol: "sftp",
  host: "demo-sftp",
  port: 22,
  user: "demo",
  startFolder: null,
  tls: "none",
  allowPlaintext: false,
  auth: { kind: "password" },
  saveSecret: false,
  ...over,
});
const ftps = (over: Partial<ConnectionProfile> = {}): ConnectionProfile =>
  profile({ name: "Demo FTPS", protocol: "ftps", host: "demo-ftp", port: 21, tls: "explicit", ...over });

const kindOf = async (promise: Promise<unknown>): Promise<string> => {
  try {
    await promise;
  } catch (error) {
    return error instanceof CoreError ? error.kind : "kein CoreError";
  }
  return "kein Fehler";
};
const jobOf = async (api: DualfileApi, id: number): Promise<JobStatus> => {
  const job = (await api.jobs()).find((candidate) => candidate.id === id);
  if (!job) throw new Error(`Auftrag ${id} fehlt`);
  return job;
};
const names = async (api: DualfileApi, path: string): Promise<string[]> =>
  (await api.list(path)).entries.map((entry) => entry.name);

/** Legt ein Profil an und verbindet es vollständig (Vertrauen „once“, Passwort „demo“). */
async function connect(api: DualfileApi, p: ConnectionProfile = profile()): Promise<{ profileId: string; connectionId: string; root: string }> {
  const { profile: saved } = await api.connectionSave(p);
  let outcome: ConnectOutcome = await api.connectionConnect(saved.id, "demo");
  if (outcome.state === "needsTrust") outcome = await api.connectionTrust(saved.id, "once", outcome.fingerprint);
  if (outcome.state !== "connected") throw new Error("nicht verbunden");
  return { profileId: saved.id, connectionId: outcome.connectionId, root: outcome.root };
}

/** Verbindet ein vorhandenes Profil erneut (ohne Pin kommt erst die Vertrauensfrage). */
async function reconnect(api: DualfileApi, profileId: string): Promise<{ connectionId: string; root: string }> {
  let outcome = await api.connectionConnect(profileId, "demo");
  if (outcome.state === "needsTrust") outcome = await api.connectionTrust(profileId, "once", outcome.fingerprint);
  if (outcome.state !== "connected") throw new Error("nicht verbunden");
  return { connectionId: outcome.connectionId, root: outcome.root };
}

describe("Profile: Anlegen, Ändern, Prüfen", () => {
  test("neues Profil bekommt eine Kennung ([a-z0-9-], höchstens 32), die Liste zeigt es ohne Pins und Geheimnis", async () => {
    const { api } = setup();
    const entry = await api.connectionSave(profile());
    expect(entry.profile.id).toMatch(/^[a-z0-9-]{1,32}$/);
    expect(entry).toMatchObject({ connected: false, connectionId: null, root: null, secretSet: false, secretStore: "none", pins: [] });
    expect(await api.connectionsList()).toEqual([entry]);
  });

  test("Ändern behält die Kennung; unbekannte Kennung wird abgelehnt", async () => {
    const { api } = setup();
    const { profile: first } = await api.connectionSave(profile());
    const changed = await api.connectionSave({ ...first, name: "Anders", port: 2222 });
    expect(changed.profile).toMatchObject({ id: first.id, name: "Anders", port: 2222 });
    expect(await api.connectionsList()).toHaveLength(1);
    expect(await kindOf(api.connectionSave({ ...first, id: "gibt-es-nicht" }))).toBe("invalidInput");
  });

  test.each<[string, Partial<ConnectionProfile>]>([
    ["leerer Name", { name: "  " }],
    ["zu langer Name", { name: "x".repeat(101) }],
    ["Steuerzeichen im Namen", { name: "a\nb" }],
    ["leerer Host", { host: "" }],
    ["Leerzeichen im Host", { host: "a b" }],
    ["Port 0", { port: 0 }],
    ["Port 65536", { port: 65_536 }],
    ["Port nicht ganzzahlig", { port: 21.5 }],
    ["leerer Benutzer", { user: "" }],
    ["relativer Startordner", { startFolder: "relativ" }],
    ["Startordner mit ..", { startFolder: "/a/../b" }],
    ["sftp mit TLS", { tls: "explicit" }],
    ["ftps ohne TLS", { protocol: "ftps", tls: "none" }],
    ["ftp mit TLS", { protocol: "ftp", tls: "implicit" }],
    ["Schlüsseldatei bei ftps", { protocol: "ftps", tls: "explicit", auth: { kind: "keyFile", path: "/k" } }],
    ["leerer Schlüsseldateipfad", { auth: { kind: "keyFile", path: "" } }],
  ])("ungültig: %s", async (_name, over) => {
    const { api } = setup();
    expect(await kindOf(api.connectionSave(profile(over)))).toBe("invalidInput");
    expect(await api.connectionsList()).toEqual([]);
  });

  test("Name eindeutig ohne Rücksicht auf Groß-/Kleinschreibung; beim Ändern des eigenen Namens kein Konflikt", async () => {
    const { api } = setup();
    const { profile: first } = await api.connectionSave(profile({ name: "Büro" }));
    expect(await kindOf(api.connectionSave(profile({ name: "BÜRO" })))).toBe("invalidInput");
    await api.connectionSave({ ...first, name: "BÜRO" });
  });

  test("Obergrenze für Profile", async () => {
    const { api } = setup();
    for (let i = 0; i < MAX_PROFILES; i += 1) await api.connectionSave(profile({ name: `p${i}` }));
    expect(await kindOf(api.connectionSave(profile({ name: "zu viel" })))).toBe("invalidInput");
  });

  test("allowPlaintext wird außerhalb von ftp verworfen; Pins aus der Eingabe werden ignoriert", async () => {
    const { api } = setup();
    const entry = await api.connectionSave({
      ...profile({ allowPlaintext: true }),
      pins: [{ protocol: "sftp", host: "x", port: 1, sha256: "a", keyType: null }],
    } as ConnectionProfile);
    expect(entry.profile.allowPlaintext).toBe(false);
    expect(entry.pins).toEqual([]);
  });
});

describe("Verbinden", () => {
  test("SFTP: needsTrust, falscher Fingerabdruck abgelehnt (Bestätigung verworfen), dann richtig, dann verbunden", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile());
    const first = await api.connectionConnect(p.id, "demo");
    expect(first).toEqual({
      state: "needsTrust",
      fingerprint: DEMO_FINGERPRINT_SFTP,
      keyType: "ssh-ed25519",
      kind: "newHostKey",
      changed: false,
    });
    expect(DEMO_FINGERPRINT_SFTP).toMatch(/^[0-9a-f]{64}$/);
    expect(await kindOf(api.connectionTrust(p.id, "once", "00".repeat(32)))).toBe("invalidInput");
    // Die Bestätigung ist verworfen: auch der richtige Fingerabdruck geht jetzt nicht mehr.
    expect(await kindOf(api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP))).toBe("invalidInput");
    const again = await api.connectionConnect(p.id, "demo");
    expect(again.state).toBe("needsTrust");
    const done = await api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP);
    expect(done).toMatchObject({ state: "connected", root: expect.stringMatching(/^sftp:[a-z0-9-]+!\/$/) });
    const [entry] = await api.connectionsList();
    expect(entry).toMatchObject({ connected: true, pins: [] });
  });

  test("FTPS: unknownIssuer ohne Schlüsseltyp; Trust ohne vorheriges Verbinden ist ungültig", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(ftps());
    expect(await kindOf(api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_FTPS))).toBe("invalidInput");
    expect(await api.connectionConnect(p.id, "demo")).toEqual({
      state: "needsTrust",
      fingerprint: DEMO_FINGERPRINT_FTPS,
      keyType: null,
      kind: "unknownIssuer",
      changed: false,
    });
    const done = await api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_FTPS);
    expect(done).toMatchObject({ state: "connected", root: expect.stringMatching(/^ftp:/) });
  });

  test("„always“ speichert den Pin, danach wird direkt verbunden; „once“ nicht; Zurücksetzen löscht ihn", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile());
    await api.connectionConnect(p.id, "demo");
    await api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP);
    expect((await api.connectionsList())[0]!.pins).toEqual([]);
    await api.connectionDisconnect(p.id);
    await api.connectionConnect(p.id, "demo");
    await api.connectionTrust(p.id, "always", DEMO_FINGERPRINT_SFTP);
    expect((await api.connectionsList())[0]!.pins).toEqual([
      { protocol: "sftp", host: "demo-sftp", port: 22, sha256: DEMO_FINGERPRINT_SFTP, keyType: "ssh-ed25519" },
    ]);
    await api.connectionDisconnect(p.id);
    expect((await api.connectionConnect(p.id, "demo")).state).toBe("connected");
    await api.connectionDisconnect(p.id);
    await api.connectionResetTrust(p.id);
    expect((await api.connectionsList())[0]!.pins).toEqual([]);
    expect((await api.connectionConnect(p.id, "demo")).state).toBe("needsTrust");
  });

  test("Pin-Abweichung: pinMismatch mit altem und neuem Wert, nie verbunden", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile({ name: "Wechsel", host: "pin-changed" }));
    const outcome = await api.connectionConnect(p.id, "demo");
    expect(outcome).toMatchObject({ state: "pinMismatch" });
    if (outcome.state === "pinMismatch") expect(outcome.old).not.toBe(outcome.new);
    expect(await kindOf(api.connectionTrust(p.id, "always", DEMO_FINGERPRINT_SFTP))).toBe("invalidInput");
    expect((await api.connectionsList())[0]).toMatchObject({ connected: false, pins: [] });
  });

  test("Sonderhosts: Zeitüberschreitung, Server aus, unbekannter Host", async () => {
    const { api } = setup();
    const kinds: string[] = [];
    for (const host of ["timeout-host", "down-host", "gibt-es-nicht"]) {
      const { profile: p } = await api.connectionSave(profile({ name: host, host }));
      kinds.push(await kindOf(api.connectionConnect(p.id, "demo")));
    }
    expect(kinds).toEqual(["timeout", "remote", "remote"]);
  });

  test("falsches Passwort: auth erst nach der Bestätigung; bei bekanntem Pin sofort und ohne ausstehende Bestätigung", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile());
    expect((await api.connectionConnect(p.id, "falsch")).state).toBe("needsTrust");
    expect(await kindOf(api.connectionTrust(p.id, "always", DEMO_FINGERPRINT_SFTP))).toBe("auth");
    expect(await kindOf(api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP))).toBe("invalidInput");
    // Pin ist gesetzt: Anmeldefehler sofort.
    expect(await kindOf(api.connectionConnect(p.id, "falsch"))).toBe("auth");
    expect(await kindOf(api.connectionConnect(p.id))).toBe("auth"); // leer zählt als falsch
    expect(await kindOf(api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP))).toBe("invalidInput");
    expect((await api.connectionConnect(p.id, "demo")).state).toBe("connected");
  });

  test("SFTP mit Schlüsseldatei braucht kein Passwort", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile({ auth: { kind: "keyFile", path: "/home/demo/.ssh/id" } }));
    await api.connectionConnect(p.id);
    expect((await api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP)).state).toBe("connected");
  });

  test("FTP ohne allowPlaintext lässt sich speichern, aber nicht verbinden; mit Erlaubnis geht es ohne Vertrauensfrage", async () => {
    const { api } = setup();
    const base = profile({ name: "Klartext", protocol: "ftp", host: "demo-ftp", port: 21 });
    const { profile: p } = await api.connectionSave(base);
    expect(await kindOf(api.connectionConnect(p.id, "demo"))).toBe("invalidInput");
    await api.connectionSave({ ...p, allowPlaintext: true });
    expect(await api.connectionConnect(p.id, "demo")).toMatchObject({ state: "connected" });
  });

  test("höchstens 8 gleichzeitige Verbindungen", async () => {
    const { api } = setup();
    for (let i = 0; i < 8; i += 1) await connect(api, profile({ name: `s${i}` }));
    const { profile: p } = await api.connectionSave(profile({ name: "neunte" }));
    expect(await kindOf(api.connectionConnect(p.id, "demo"))).toBe("tooManyConnections");
  });

  test("Startordner: Wurzel ist der Startordner; ein fehlender Ordner scheitert ohne Verbindung", async () => {
    const { api } = setup();
    const ok = await connect(api, profile({ startFolder: "/dokumente" }));
    expect(ok.root).toMatch(/^sftp:[a-z0-9-]+!\/dokumente$/);
    expect(await names(api, ok.root)).toContain("bericht.txt");
    const { profile: bad } = await api.connectionSave(profile({ name: "kaputt", startFolder: "/nix" }));
    await api.connectionConnect(bad.id, "demo");
    expect(await kindOf(api.connectionTrust(bad.id, "once", DEMO_FINGERPRINT_SFTP))).toBe("notFound");
    expect((await api.connectionsList()).find((e) => e.profile.id === bad.id)?.connected).toBe(false);
  });

  test("erneutes Verbinden eines verbundenen Profils liefert dieselbe Verbindung", async () => {
    const { api } = setup();
    const c = await connect(api);
    expect(await api.connectionConnect(c.profileId, "demo")).toMatchObject({ state: "connected", connectionId: c.connectionId });
  });
});

describe("Geheimnisse", () => {
  test("das Passwort taucht in keiner Antwort, keinem Eintrag und keinem Fehlertext auf", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile({ saveSecret: true }));
    const seen: unknown[] = [];
    seen.push(await api.connectionConnect(p.id, MARKER));
    try {
      await api.connectionTrust(p.id, "always", DEMO_FINGERPRINT_SFTP); // Auth schlägt fehl (Marker ≠ Demo-Passwort)
    } catch (error) {
      seen.push((error as Error).message, (error as CoreError).kind);
    }
    seen.push(await api.connectionsList(), await api.secretSet(p.id, MARKER), await api.connectionsList(), await api.jobs());
    for (const wrong of [MARKER]) {
      try {
        await api.connectionConnect(p.id, wrong);
      } catch (error) {
        seen.push((error as Error).message);
      }
    }
    expect(JSON.stringify(seen)).not.toContain(MARKER);
    // Auch das richtige Passwort steht nirgends in Listen.
    await api.secretSet(p.id, "demo");
    expect(JSON.stringify(await api.connectionsList())).not.toMatch(/secret"?:\s*"/);
  });

  test("saveSecret: nach erfolgreichem Verbinden ist das Geheimnis gesetzt und wird wiederverwendet", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile({ saveSecret: true }));
    await api.connectionConnect(p.id, "demo");
    const outcome = await api.connectionTrust(p.id, "always", DEMO_FINGERPRINT_SFTP);
    expect(outcome).toMatchObject({ state: "connected", secretStore: "keychain" });
    expect((await api.connectionsList())[0]).toMatchObject({ secretSet: true, secretStore: "keychain" });
    await api.connectionDisconnect(p.id);
    expect((await api.connectionConnect(p.id)).state).toBe("connected"); // ohne Eingabe
  });

  test("ohne saveSecret wird nichts gemerkt", async () => {
    const { api } = setup();
    const c = await connect(api);
    expect((await api.connectionsList())[0]).toMatchObject({ secretSet: false, secretStore: "none" });
    await api.connectionDisconnect(c.profileId);
    await api.connectionConnect(c.profileId);
    expect(await kindOf(api.connectionTrust(c.profileId, "once", DEMO_FINGERPRINT_SFTP))).toBe("auth");
  });

  test("secretSet / secretClear; ohne Schlüsselbund nur „session“", async () => {
    const { api } = setup({ keychain: false });
    const { profile: p } = await api.connectionSave(profile());
    expect(await api.secretSet(p.id, "demo")).toBe("session");
    expect((await api.connectionsList())[0]).toMatchObject({ secretSet: true, secretStore: "session" });
    await api.secretClear(p.id);
    expect((await api.connectionsList())[0]).toMatchObject({ secretSet: false, secretStore: "none" });
    expect(await setup().api.secretSet("unbekannt", "x").catch((e: CoreError) => e.kind)).toBe("invalidInput");
  });

  test("saveSecret auf false im Profil räumt das gemerkte Geheimnis ab", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile({ saveSecret: true }));
    await api.secretSet(p.id, "demo");
    await api.connectionSave({ ...p, saveSecret: false });
    expect((await api.connectionsList())[0]!.secretSet).toBe(false);
  });
});

describe("Trennen, Löschen, Verbindung verloren", () => {
  const probes = (api: DualfileApi, root: string): Record<string, () => Promise<unknown>> => ({
    list: () => api.list(root),
    fileInfo: () => api.fileInfo(`${root}/readme.md`),
    readBlock: () => api.readBlock(`${root}/readme.md`, 0, 10),
    findInFile: () => api.findInFile(`${root}/readme.md`, new Uint8Array([65]), 0, "forward", false),
    copyFrom: () => api.copy([`${root}/readme.md`], `${HOME}/Musik`),
    copyTo: () => api.copy([`${HOME}/Dokumente/notizen.txt`], root),
    move: () => api.move([`${root}/readme.md`], `${HOME}/Musik`),
    rename: () => api.rename(`${root}/readme.md`, "neu.md"),
    makeDir: () => api.makeDir(root, "neu"),
    planDelete: () => api.planDelete([`${root}/readme.md`]),
    compare: () => api.compareDirs(root, `${HOME}/Musik`, { mode: "sizeTime", recursive: false }),
  });

  test("nach dem Trennen liefern alle Befehle auf Pfade der Verbindung „disconnected“", async () => {
    const { api } = setup();
    const c = await connect(api);
    const calls = probes(api, c.root);
    await api.connectionDisconnect(c.profileId);
    for (const [name, run] of Object.entries(calls)) expect([name, await kindOf(run())]).toEqual([name, "disconnected"]);
    expect(await kindOf(api.list(`conn:${c.profileId}!/`))).toBe("disconnected");
    expect((await api.connectionsList())[0]).toMatchObject({ connected: false, connectionId: null, root: null });
  });

  test("dropConnection (Verbindung verloren) wirkt wie Trennen", async () => {
    const { api } = setup();
    const c = await connect(api);
    const calls = probes(api, c.root);
    api.dropConnection(c.connectionId);
    for (const [name, run] of Object.entries(calls)) expect([name, await kindOf(run())]).toEqual([name, "disconnected"]);
    expect((await api.connectionsList())[0]!.connected).toBe(false);
  });

  test("nach neuem Verbinden hat die alte Verbindungs-ID keine Wirkung mehr", async () => {
    const { api } = setup();
    const c = await connect(api);
    await api.connectionDisconnect(c.profileId);
    const next = await reconnect(api, c.profileId);
    expect(next.connectionId).not.toBe(c.connectionId);
    expect(await kindOf(api.list(c.root))).toBe("disconnected");
    expect(await names(api, next.root)).toContain("readme.md");
  });

  test("Löschen des Profils trennt, räumt das Geheimnis ab und entfernt das Profil", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile({ saveSecret: true }));
    await api.connectionConnect(p.id, "demo");
    const outcome = await api.connectionTrust(p.id, "always", DEMO_FINGERPRINT_SFTP);
    if (outcome.state !== "connected") throw new Error("nicht verbunden");
    await api.connectionDelete(p.id);
    expect(await api.connectionsList()).toEqual([]);
    expect(await kindOf(api.list(outcome.root))).toBe("disconnected");
    expect(await kindOf(api.connectionConnect(p.id, "demo"))).toBe("invalidInput");
    // Ein neues Profil mit gleichem Namen erbt weder Geheimnis noch Pin.
    const fresh = await api.connectionSave(profile({ saveSecret: true }));
    expect(fresh).toMatchObject({ secretSet: false, pins: [] });
  });

  test("conn:-Pfade: getrennt → disconnected, verbunden → löst auf die aktuelle Verbindung auf", async () => {
    const { api } = setup();
    const { profile: p } = await api.connectionSave(profile());
    expect(await kindOf(api.list(`conn:${p.id}!/`))).toBe("disconnected");
    expect(await kindOf(api.list("conn:unbekannt!/"))).toBe("disconnected");
    await api.connectionConnect(p.id, "demo");
    await api.connectionTrust(p.id, "once", DEMO_FINGERPRINT_SFTP);
    const listing = await api.list(`conn:${p.id}!/dokumente`);
    expect(listing.path).toMatch(/^sftp:[a-z0-9-]+!\/dokumente$/);
    expect(listing.entries.map((e) => e.name)).toContain("bericht.txt");
  });
});

describe("Einstellungen", () => {
  test("Serverorte werden beim Speichern abgelehnt, conn:-Pfade sind erlaubt", async () => {
    const { api } = setup();
    const { settings } = await api.loadSettings();
    const withPath = (path: string) => ({
      ...settings,
      bookmarks: [{ name: "x", path, color: "#fff", order: 0 }],
    });
    expect(await kindOf(api.saveSettings(withPath("sftp:c1!/a")))).toBe("invalidSettings");
    expect(await kindOf(api.saveSettings(withPath("ftp:c1!/a")))).toBe("invalidSettings");
    await api.saveSettings(withPath("conn:profile-1!/a"));
    await api.saveSettings(withPath("/home/demo"));
  });

  test("Einstellungen enthalten keine Verbindungsprofile", async () => {
    const { api } = setup();
    await api.connectionSave(profile());
    const { settings } = await api.loadSettings();
    expect(JSON.stringify(settings)).not.toContain("Demo SFTP");
  });
});

describe("Operationen auf dem Server", () => {
  test("Auflisten, Ordner anlegen, Umbenennen; Namen mit .. und / werden abgelehnt", async () => {
    const { api } = setup();
    const c = await connect(api);
    expect(await names(api, c.root)).toEqual(["dokumente", "projekte", "backup.bin", "readme.md"]);
    const dir = await api.makeDir(c.root, "neu");
    expect(dir).toBe(`${c.root}neu`);
    expect(await names(api, c.root)).toContain("neu");
    expect(await kindOf(api.makeDir(c.root, ".."))).toBe("invalidName");
    expect(await kindOf(api.makeDir(c.root, "a/b"))).toBe("invalidName");
    expect(await kindOf(api.makeDir(c.root, "neu"))).toBe("exists");
    const renamed = await api.rename(`${c.root}readme.md`, "lies-mich.md");
    expect(renamed.newPath).toBe(`${c.root}lies-mich.md`);
    expect(await kindOf(api.rename(`${c.root}lies-mich.md`, "../x"))).toBe("invalidName");
    expect(await kindOf(api.rename(c.root, "x"))).toBe("invalidPath");
  });

  test("Datei lesen, Info und Suche arbeiten über den Server", async () => {
    const { api } = setup();
    const c = await connect(api);
    const info = await api.fileInfo(`${c.root}readme.md`);
    expect(info.size).toBe(700);
    expect((await api.readBlock(`${c.root}readme.md`, 0, 16)).length).toBe(16);
    const hit = await api.findInFile(`${c.root}readme.md`, new TextEncoder().encode("Zeile"), 0, "forward", false);
    expect(hit.kind).toBe("found");
  });

  test("Kopieren Server → lokal und lokal → Server", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    const down = await api.copy([`${c.root}dokumente/bericht.txt`], `${HOME}/Musik`);
    await scheduler.runAll();
    expect((await jobOf(api, down)).state).toBe("done");
    expect(await names(api, `${HOME}/Musik`)).toEqual(["bericht.txt"]);
    const up = await api.copy([`${HOME}/Dokumente/notizen.txt`], `${c.root}dokumente`);
    await scheduler.runAll();
    expect((await jobOf(api, up)).state).toBe("done");
    expect(await names(api, `${c.root}dokumente`)).toContain("notizen.txt");
  });

  test("Server bleibt über Trennen und neues Verbinden erhalten", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    await api.copy([`${HOME}/Dokumente/notizen.txt`], c.root);
    await scheduler.runAll();
    await api.connectionDisconnect(c.profileId);
    const again = await reconnect(api, c.profileId);
    expect(await names(api, again.root)).toContain("notizen.txt");
  });

  test("Kopieren innerhalb des Servers, Verschieben Server → lokal entfernt die Quelle", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    const inside = await api.copy([`${c.root}readme.md`], `${c.root}projekte`);
    await scheduler.runAll();
    expect((await jobOf(api, inside)).state).toBe("done");
    expect(await names(api, `${c.root}projekte`)).toContain("readme.md");
    expect(await names(api, c.root)).toContain("readme.md");
    const moved = await api.move([`${c.root}dokumente/tabelle.csv`], `${HOME}/Musik`);
    await scheduler.runAll();
    expect((await jobOf(api, moved)).state).toBe("done");
    expect(await names(api, `${c.root}dokumente`)).not.toContain("tabelle.csv");
    expect(await names(api, `${HOME}/Musik`)).toContain("tabelle.csv");
  });

  test("Konflikt am Server: nie überschreiben ohne Antwort", async () => {
    const { api, scheduler, events } = setup();
    const c = await connect(api);
    const id = await api.copy([`${HOME}/Dokumente/notizen.txt`], `${c.root}dokumente`);
    await scheduler.runAll();
    const first = await api.copy([`${HOME}/Dokumente/notizen.txt`], `${c.root}dokumente`);
    await scheduler.runAll();
    const conflict = events.find((e) => e.type === "conflict" && e.conflict.jobId === first);
    expect(conflict).toBeDefined();
    expect((await jobOf(api, first)).state).not.toBe("done");
    await api.answerConflict(first, { resolution: "skip", applyToAll: false });
    await scheduler.runAll();
    expect((await jobOf(api, first)).state).toBe("done");
    expect((await jobOf(api, first)).skipped).toHaveLength(1);
    expect((await jobOf(api, id)).state).toBe("done");
  });

  test("zwischen zwei Servern wird nicht kopiert", async () => {
    const { api } = setup();
    const a = await connect(api);
    const b = await connect(api, ftps({ allowPlaintext: false }));
    expect(await kindOf(api.copy([`${a.root}readme.md`], `${b.root}pub`))).toBe("unsupported");
  });

  test("Löschen: Plan + Token am Server; Token einer neu aufgebauten Verbindung ist ungültig, nichts wird gelöscht", async () => {
    const { api } = setup();
    const c = await connect(api);
    const plan = await api.planDelete([`${c.root}readme.md`]);
    const job = await api.deletePermanent(plan, plan.token);
    expect((await jobOf(api, job)).state).toBe("done");
    expect(await names(api, c.root)).not.toContain("readme.md");
    expect(await kindOf(api.deletePermanent(plan, plan.token))).toBe("invalidToken");

    const old = await api.planDelete([`${c.root}backup.bin`]);
    await api.connectionDisconnect(c.profileId);
    const next = await reconnect(api, c.profileId);
    expect(await kindOf(api.deletePermanent(old, old.token))).toBe("invalidToken");
    expect(await names(api, next.root)).toContain("backup.bin");
  });

  test("Löschen nach Trennen ohne neue Verbindung: disconnected", async () => {
    const { api } = setup();
    const c = await connect(api);
    const plan = await api.planDelete([`${c.root}backup.bin`]);
    await api.connectionDisconnect(c.profileId);
    expect(await kindOf(api.deletePermanent(plan, plan.token))).toBe("disconnected");
  });

  test("Token einer anderen Verbindung: Plan über Server, Löschen lokaler Dinge nicht vermischt", async () => {
    const { api } = setup();
    const c = await connect(api);
    expect(await kindOf(api.planDelete([`${c.root}readme.md`, `${HOME}/Dokumente/notizen.txt`]))).toBe("invalidInput");
    expect(await kindOf(api.planDelete([c.root]))).toBe("invalidPath");
  });

  test("Papierkorb, Öffnen, Packen auf Server, Abgleich auf Server sind „unsupported“; Server als Quelle geht", async () => {
    const { api, scheduler } = setup({});
    const c = await connect(api);
    expect(await kindOf(api.trash([`${c.root}readme.md`]))).toBe("unsupported");
    expect(await kindOf(api.restoreFromTrash([`${c.root}readme.md`]))).toBe("unsupported");
    expect(await kindOf(api.openExternal(`${c.root}readme.md`))).toBe("unsupported");
    expect(await names(api, c.root)).toContain("readme.md"); // nichts wurde stattdessen gelöscht
    expect(await kindOf(api.packArchive([`${HOME}/Dokumente/notizen.txt`], c.root, "a.zip", "zip"))).toBe("unsupported");
    // Packen mit Server als Quelle
    const pack = await api.packArchive([`${c.root}readme.md`], `${HOME}/Musik`, "a.zip", "zip");
    await scheduler.runAll();
    expect((await jobOf(api, pack)).state).toBe("done");
    // Abgleich: Server als Ziel scheitert, Server als Quelle geht
    const toServer = await api.compareDirs(`${HOME}/Dokumente`, `${c.root}dokumente`, { mode: "sizeTime", recursive: true });
    await scheduler.runAll();
    expect(await kindOf(api.planSync(toServer, "leftToRight", { replaceNewer: false }))).toBe("unsupported");
    const fromServer = await api.compareDirs(`${c.root}dokumente`, `${HOME}/Musik`, { mode: "sizeTime", recursive: true });
    await scheduler.runAll();
    const plan = await api.planSync(fromServer, "leftToRight", { replaceNewer: false });
    expect(plan.copies).toBeGreaterThan(0);
  });

  test("watch ignoriert Serverpfade: kein Ereignis, kein Fehler", async () => {
    const { api, events } = setup();
    const c = await connect(api);
    await api.watch([c.root]);
    expect(events).toEqual([]);
  });
});

describe("Übertragungen: Abbruch und Fortsetzen", () => {
  const chunkJob = async (api: MockApi, scheduler: ManualScheduler, from: string, to: string): Promise<number> => {
    const id = await api.copy([from], to);
    await scheduler.step(); // Start
    await scheduler.step(); // erster Block
    return id;
  };

  test("Abbruch beim Kopieren von/zu einem Server lässt eine Teildatei samt Fortsetz-Hinweis zurück", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    const id = await chunkJob(api, scheduler, `${c.root}backup.bin`, `${HOME}/Musik`);
    await api.cancelJob(id);
    await scheduler.runAll();
    const job = await jobOf(api, id);
    expect(job.state).toBe("cancelled");
    expect(job.warnings).toHaveLength(1);
    expect(job.warnings[0]!.message).toContain("fortgesetzt");
    expect(await names(api, `${HOME}/Musik`)).toEqual(["backup.bin.dualfile-part"]);
  });

  test("erneutes Kopieren setzt fort: Teildatei verschwindet, Fortschritt zählt ab dem Offset", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    const first = await chunkJob(api, scheduler, `${c.root}backup.bin`, `${HOME}/Musik`);
    const written = (await jobOf(api, first)).bytesDone;
    await api.cancelJob(first);
    await scheduler.runAll();
    const second = await api.copy([`${c.root}backup.bin`], `${HOME}/Musik`);
    await scheduler.runAll();
    const job = await jobOf(api, second);
    expect(job.state).toBe("done");
    expect(job.warnings.some((w) => w.message.startsWith(`Fortgesetzt: backup.bin ab ${written} `))).toBe(true);
    expect(job.bytesDone).toBe(job.bytesTotal);
    expect(await names(api, `${HOME}/Musik`)).toEqual(["backup.bin"]);
  });

  test("Abbruch beim Hochladen: Teildatei auf dem Server, Fortsetzen nach neuem Verbinden", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api, ftps());
    const id = await chunkJob(api, scheduler, `${HOME}/Downloads/setup.AppImage`, `${c.root}upload`);
    await api.cancelJob(id);
    await scheduler.runAll();
    expect(await names(api, `${c.root}upload`)).toEqual(["setup.AppImage.dualfile-part"]);
    await api.connectionDisconnect(c.profileId);
    const next = await reconnect(api, c.profileId);
    const again = await api.copy([`${HOME}/Downloads/setup.AppImage`], `${next.root}upload`);
    await scheduler.runAll();
    const job = await jobOf(api, again);
    expect(job.state).toBe("done");
    expect(job.warnings.some((w) => w.message.startsWith("Fortgesetzt"))).toBe(true);
    expect(await names(api, `${next.root}upload`)).toEqual(["setup.AppImage"]);
  });

  test("Verbindung reißt mitten im Auftrag ab: Auftrag scheitert mit Hinweis, Teildatei bleibt", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    const id = await chunkJob(api, scheduler, `${c.root}backup.bin`, `${HOME}/Musik`);
    api.dropConnection(c.connectionId);
    await scheduler.runAll();
    const job = await jobOf(api, id);
    expect(job.state).toBe("failed");
    expect(job.failed[0]!.message).toMatch(/getrennt/);
    expect(job.warnings[0]!.message).toContain("fortgesetzt");
    expect(await names(api, `${HOME}/Musik`)).toEqual(["backup.bin.dualfile-part"]);
  });

  test("fremde Hilfsdatei gleichen Namens wird nie überschrieben", async () => {
    const { api, scheduler } = setup();
    const c = await connect(api);
    const fremd = await api.makeDir(`${HOME}/Musik`, "backup.bin.dualfile-part");
    const id = await api.copy([`${c.root}backup.bin`], `${HOME}/Musik`);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("failed");
    expect(await names(api, fremd)).toEqual([]);
    expect(await names(api, `${HOME}/Musik`)).toEqual(["backup.bin.dualfile-part"]);
  });

  test("rein lokale Abbrüche hinterlassen keine Teildatei und keine Warnung", async () => {
    const { api, scheduler } = setup();
    const id = await chunkJob(api, scheduler, `${HOME}/Downloads/setup.AppImage`, `${HOME}/Musik`);
    await api.cancelJob(id);
    await scheduler.runAll();
    expect((await jobOf(api, id)).warnings).toEqual([]);
    expect(await names(api, `${HOME}/Musik`)).toEqual([]);
  });
});

test("remote operations and downloads never create undo history", async () => {
  const { api, scheduler } = setup();
  const { root } = await connect(api);
  await api.makeDir(root, "undo-excluded");
  await api.rename(`${root}/undo-excluded`, "renamed-excluded");
  await api.copy([`${HOME}/Dokumente/notizen.txt`], root);
  await scheduler.runAll();
  const folder = await api.makeDir(HOME, "download-undo-excluded");
  const before = (await api.undoList()).length;
  await api.copy([`${root}/notizen.txt`], folder);
  await scheduler.runAll();
  expect(await api.undoList()).toHaveLength(before);
  expect(before).toBe(1);
});
