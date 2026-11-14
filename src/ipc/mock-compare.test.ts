import { describe, expect, test } from "vitest";
import { CoreError, type DualfileApi } from "./api";
import { createMockApi, type MockScheduler } from "./mock";
import { diffBytes } from "./mock-diff";
import {
  DIFF_MAX_BYTES,
  MAX_COMPARES,
  MAX_SYNC_PLANS,
  MSG_DIFF_BINARY,
  MSG_DIFF_TOO_LARGE,
  type CompareOptions,
  type CompareState,
  type CoreEvent,
  type JobStatus,
} from "./types";

const HOME = "/home/demo";
const LEFT = `${HOME}/Vergleich/links`;
const RIGHT = `${HOME}/Vergleich/rechts`;
const TEXT = `${HOME}/Vergleich/text`;
const SIZE_TIME: CompareOptions = { mode: "sizeTime", recursive: true };
const CONTENT: CompareOptions = { mode: "content", recursive: true };

/** Von Hand gesteuerter Zeitgeber wie in `mock.test.ts`. */
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

function setup(): { api: DualfileApi; scheduler: ManualScheduler; events: CoreEvent[] } {
  const scheduler = new ManualScheduler();
  const api = createMockApi({ scheduler, now: () => Date.UTC(2026, 10, 12, 12, 0), compareDemo: true });
  const events: CoreEvent[] = [];
  api.onEvent((event) => events.push(event));
  return { api, scheduler, events };
}

const jobOf = async (api: DualfileApi, id: number): Promise<JobStatus> => {
  const job = (await api.jobs()).find((candidate) => candidate.id === id);
  if (!job) throw new Error(`Auftrag ${id} fehlt`);
  return job;
};

async function compare(
  s: ReturnType<typeof setup>,
  left = LEFT,
  right = RIGHT,
  options: CompareOptions = SIZE_TIME,
): Promise<number> {
  const id = await s.api.compareDirs(left, right, options);
  await s.scheduler.runAll();
  expect((await jobOf(s.api, id)).state).toBe("done");
  return id;
}

const rejects = async (promise: Promise<unknown>, kind: string): Promise<CoreError> => {
  const error = await promise.then(
    () => null,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  expect((error as CoreError).kind).toBe(kind);
  return error as CoreError;
};

const names = async (api: DualfileApi, path: string): Promise<string[]> =>
  (await api.list(path)).entries.map((entry) => entry.name);

describe("compareDirs / comparePage", () => {
  test("die Demo zeigt jeden Zustand, einen unbekannten Teilbaum und ein unvollständiges Ergebnis", async () => {
    const s = setup();
    const id = await compare(s);
    const page = await s.api.comparePage(id, 0, 1000);
    const state = (rel: string): CompareState => page.entries.find((e) => e.rel === rel)!.state;
    expect(state("gleich.txt")).toBe("same");
    expect(state("nur-links.txt")).toBe("onlyLeft");
    expect(state("nur-rechts.txt")).toBe("onlyRight");
    expect(state("neuer-links.txt")).toBe("leftNewer");
    expect(state("neuer-rechts.txt")).toBe("rightNewer");
    expect(state("anders.txt")).toBe("different"); // Zeit innerhalb von 2 s, Größe anders
    expect(state("typ")).toBe("typeMismatch");
    expect(state("verknuepfung")).toBe("same");
    expect(state("verknuepfung-anders")).toBe("different");
    expect(state("gesperrt")).toBe("unknown");
    expect(page.entries.find((e) => e.rel === "gesperrt")!.note).toBe("Zugriff verweigert");
    // nie „leer“ oder „fehlt“: darunter steht nichts
    expect(page.entries.some((e) => e.rel.startsWith("gesperrt/"))).toBe(false);
    expect(state("ordner-nur-links/a.txt")).toBe("onlyLeft");
    expect(state("gemeinsam/b.txt")).toBe("leftNewer");
    expect(page.entries.find((e) => e.rel === "ordner-nur-links")!.listed).toBe(true);
    expect(page.entries.find((e) => e.rel === "typ")!.listed).toBe(false);

    expect(page.summary).toMatchObject({ left: LEFT, right: RIGHT, mode: "sizeTime", recursive: true, entries: 21 });
    expect(page.summary.counts).toEqual({
      onlyLeft: 5,
      onlyRight: 4,
      same: 5,
      leftNewer: 2,
      rightNewer: 1,
      different: 2,
      typeMismatch: 1,
      unknown: 1,
    });
    expect(page.summary.incomplete).toBe(true);
    expect(page.summary.issues).toEqual(["gesperrt: Zugriff verweigert"]);
    // Ordner vor Inhalt, Geschwister nach Namen
    const rels = page.entries.map((e) => e.rel);
    expect(rels.indexOf("gemeinsam")).toBeLessThan(rels.indexOf("gemeinsam/a.txt"));
    expect(rels.indexOf("gemeinsam/d.txt")).toBeLessThan(rels.indexOf("gesperrt"));
  });

  test("Inhaltsvergleich findet gleich große, gleich alte Dateien mit anderem Inhalt", async () => {
    const s = setup();
    const bySize = await s.api.comparePage(await compare(s), 0, 1000);
    expect(bySize.entries.find((e) => e.rel === "gleiche-groesse.txt")!.state).toBe("same");
    const byContent = await s.api.comparePage(await compare(s, LEFT, RIGHT, CONTENT), 0, 1000);
    expect(byContent.entries.find((e) => e.rel === "gleiche-groesse.txt")!.state).toBe("different");
    expect(byContent.summary.mode).toBe("content");
  });

  test("ohne Unterordner wird nicht abgestiegen", async () => {
    const s = setup();
    const page = await s.api.comparePage(await compare(s, LEFT, RIGHT, { mode: "sizeTime", recursive: false }), 0, 1000);
    expect(page.entries.some((e) => e.rel.includes("/"))).toBe(false);
    expect(page.summary.recursive).toBe(false);
    expect(page.entries.find((e) => e.rel === "gemeinsam")!.listed).toBe(false);
  });

  test("Seiten, Filter nach Zustand und Text, Gesamtübersicht bleibt", async () => {
    const s = setup();
    const id = await compare(s);
    const first = await s.api.comparePage(id, 0, 5);
    const second = await s.api.comparePage(id, 5, 5);
    expect(first.entries).toHaveLength(5);
    expect(first.total).toBe(21);
    expect(second.entries[0]!.rel).not.toBe(first.entries[0]!.rel);
    expect((await s.api.comparePage(id, 1000, 5)).entries).toEqual([]);
    const only = await s.api.comparePage(id, 0, 100, { states: ["onlyRight", "unknown"] });
    expect(only.total).toBe(5);
    expect(only.summary.entries).toBe(21);
    expect(only.entries.every((e) => e.state === "onlyRight" || e.state === "unknown")).toBe(true);
    expect((await s.api.comparePage(id, 0, 100, { states: [], text: "NUR-" })).entries.map((e) => e.rel)).toEqual([
      "nur-links.txt",
      "nur-rechts.txt",
      "ordner-nur-links",
      "ordner-nur-links/a.txt",
      "ordner-nur-links/b.txt",
      "ordner-nur-rechts",
      "ordner-nur-rechts/r.txt",
    ]);
    // Eine Seite ist nie größer als erlaubt, auch bei großem Limit
    expect((await s.api.comparePage(id, 0, 1_000_000)).entries).toHaveLength(21);
    // Die Antwort ist eine Kopie: Ändern wirkt nicht auf das Ergebnis
    first.entries[0]!.state = "unknown";
    expect((await s.api.comparePage(id, 0, 5)).entries[0]!.state).not.toBe("unknown");
  });

  test("ungültige Eingaben und unbekannte Kennungen", async () => {
    const s = setup();
    const id = await compare(s);
    await rejects(s.api.comparePage(id, -1, 5), "invalidInput");
    await rejects(s.api.comparePage(id, 0, 1.5), "invalidInput");
    await rejects(s.api.comparePage(id, 0, 5, { text: "a\0b" }), "invalidInput");
    await rejects(s.api.comparePage(id, 0, 5, { text: "x".repeat(2000) }), "invalidInput");
    await rejects(s.api.comparePage(id + 99, 0, 5), "notFound");
    await rejects(s.api.compareClose(id + 99), "notFound");
    await rejects(s.api.compareDirs("relativ", RIGHT, SIZE_TIME), "invalidPath");
    await rejects(s.api.compareDirs(LEFT, `${RIGHT}/fehlt`, SIZE_TIME), "notFound");
    await rejects(s.api.compareDirs(LEFT, `${RIGHT}/gleich.txt`, SIZE_TIME), "notDirectory");
    await rejects(s.api.compareDirs(LEFT, RIGHT, { mode: "x" as "content", recursive: true }), "invalidInput");
  });

  test("Auftrag mit Fortschritt; Schließen gibt das Ergebnis frei; höchstens vier gleichzeitig", async () => {
    const s = setup();
    const id = await s.api.compareDirs(LEFT, RIGHT, SIZE_TIME);
    expect((await jobOf(s.api, id)).name).toBe("Vergleichen: links ↔ rechts");
    // noch nicht fertig: kein Ergebnis
    await rejects(s.api.comparePage(id, 0, 5), "notFound");
    await s.scheduler.runAll();
    expect((await jobOf(s.api, id)).state).toBe("done");
    const ids = [id];
    for (let i = 1; i < MAX_COMPARES; i += 1) ids.push(await compare(s));
    await rejects(s.api.compareDirs(LEFT, RIGHT, SIZE_TIME), "invalidInput");
    await s.api.compareClose(ids[0]!);
    await rejects(s.api.comparePage(ids[0]!, 0, 5), "notFound");
    const again = await compare(s);
    expect((await s.api.comparePage(again, 0, 1)).entries).toHaveLength(1);
  });

  test("laufende Vergleiche zählen mit; ein Abbruch hinterlässt kein Ergebnis und gibt den Platz frei", async () => {
    const s = setup();
    const running: number[] = [];
    for (let i = 0; i < MAX_COMPARES; i += 1) running.push(await s.api.compareDirs(LEFT, RIGHT, SIZE_TIME));
    await rejects(s.api.compareDirs(LEFT, RIGHT, SIZE_TIME), "invalidInput");
    await s.api.cancelJob(running[0]!);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, running[0]!)).state).toBe("cancelled");
    await rejects(s.api.comparePage(running[0]!, 0, 5), "notFound");
    expect((await jobOf(s.api, running[1]!)).state).toBe("done");
    // Platz wieder frei
    await compare(s);
  });
});

describe("planSync / runSync", () => {
  test("Plan: nur Fehlendes, Zähler, Hinweise, nichts unter unbekannten Teilen; ändert noch nichts", async () => {
    const s = setup();
    const id = await compare(s);
    const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: false });
    expect(plan).toMatchObject({ mode: "leftToRight", source: LEFT, target: RIGHT, incomplete: true });
    expect([plan.makeDirs, plan.copies, plan.replaces, plan.trashes]).toEqual([1, 4, 0, 0]);
    expect(plan.totalActions).toBe(5);
    expect(plan.actionsTruncated).toBe(false);
    expect(plan.bytes).toBe(70 + 300 + 100 + 200);
    expect(plan.actions.map((a) => `${a.kind}:${a.rel}`)).toEqual([
      "copy:gemeinsam/c.txt",
      "copy:nur-links.txt",
      "makeDir:ordner-nur-links",
      "copy:ordner-nur-links/a.txt",
      "copy:ordner-nur-links/b.txt",
    ]);
    expect(plan.actions.every((a, i) => a.index === i)).toBe(true);
    expect(plan.actions.some((a) => a.rel.startsWith("gesperrt"))).toBe(false);
    const reasons = Object.fromEntries(plan.notes.map((n) => [n.rel, n.reason]));
    expect(reasons.typ).toMatch(/passen nicht zusammen/);
    expect(reasons["gemeinsam/b.txt"]).toMatch(/Quelle ist neuer/);
    expect(reasons["neuer-rechts.txt"]).toMatch(/Ziel ist neuer/);
    expect(reasons["verknuepfung-anders"]).toMatch(/Verknüpfungen/);
    expect(plan.notesTotal).toBe(plan.notes.length);
    expect(plan.token).not.toBe("");
    // geplant ist noch nichts passiert
    expect(await names(s.api, RIGHT)).not.toContain("nur-links.txt");
  });

  test("Rechts nach links und Spiegeln: bei unvollständigem Ergebnis nie Löschen", async () => {
    const s = setup();
    const id = await compare(s);
    const back = await s.api.planSync(id, "rightToLeft", { replaceNewer: false });
    expect(back).toMatchObject({ source: RIGHT, target: LEFT });
    expect(back.actions.map((a) => a.rel)).toContain("nur-rechts.txt");
    const mirror = await s.api.planSync(id, "mirror", { replaceNewer: true });
    expect(mirror.trashes).toBe(0);
    expect(mirror.incomplete).toBe(true);
    expect(mirror.notes.some((n) => n.rel === "" && /nichts gelöscht/.test(n.reason))).toBe(true);
  });

  test("Spiegeln eines vollständigen Vergleichs: Löschen nur Eintrag für Eintrag in den Papierkorb", async () => {
    const s = setup();
    const id = await compare(s, `${LEFT}/ordner-nur-links`, `${RIGHT}/ordner-nur-rechts`);
    const plan = await s.api.planSync(id, "mirror", { replaceNewer: false });
    expect(plan.incomplete).toBe(false);
    expect(plan.actions.map((a) => `${a.kind}:${a.rel}`)).toEqual(["copy:a.txt", "copy:b.txt", "trash:r.txt"]);
    expect(plan.trashes).toBe(1);
    const job = await s.api.runSync(plan.token);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, job)).state).toBe("done");
    expect(await names(s.api, `${RIGHT}/ordner-nur-rechts`)).toEqual(["a.txt", "b.txt"]);
    // endgültig weg ist nichts: es liegt im Papierkorb
    await s.api.restoreFromTrash([`${RIGHT}/ordner-nur-rechts/r.txt`]);
    expect(await names(s.api, `${RIGHT}/ordner-nur-rechts`)).toContain("r.txt");
  });

  test("Ausführen: nur mit dem Token des Plans, nur einmal; Ziel erhält genau die Plan-Einträge", async () => {
    const s = setup();
    const id = await compare(s);
    const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: false });
    for (const bad of ["", "abc", "sync-1-xxxx", `${plan.token}x`, "del-1-abc"]) {
      await rejects(s.api.runSync(bad), "invalidToken");
    }
    expect(await names(s.api, RIGHT)).not.toContain("nur-links.txt");
    const job = await s.api.runSync(plan.token);
    expect((await jobOf(s.api, job)).name).toBe("Synchronisieren: links");
    await s.scheduler.runAll();
    const status = await jobOf(s.api, job);
    expect(status).toMatchObject({ state: "done", filesDone: 5 });
    expect(await names(s.api, RIGHT)).toContain("nur-links.txt");
    expect(await names(s.api, `${RIGHT}/ordner-nur-links`)).toEqual(["a.txt", "b.txt"]);
    expect(await names(s.api, `${RIGHT}/gemeinsam`)).toContain("c.txt");
    // nicht im Plan: unverändert
    expect(await names(s.api, `${RIGHT}/gesperrt`)).toEqual(["daten.txt"]);
    expect(await names(s.api, RIGHT)).toContain("nur-rechts.txt");
    expect(s.events.some((e) => e.type === "dirChanged" && e.path === RIGHT)).toBe(true);
    // verbraucht
    await rejects(s.api.runSync(plan.token), "invalidToken");
  });

  test("Auswahl behält nur die gewählten Aktionen samt Ordner und lehnt Fremdes ab", async () => {
    const s = setup();
    const id = await compare(s);
    const options = { replaceNewer: false };
    await rejects(s.api.planSync(id, "leftToRight", options, ["gibt-es-nicht"]), "invalidInput");
    const plan = await s.api.planSync(id, "leftToRight", options, ["ordner-nur-links/a.txt"]);
    expect(plan.actions.map((a) => `${a.kind}:${a.rel}`)).toEqual(["makeDir:ordner-nur-links", "copy:ordner-nur-links/a.txt"]);
    expect(plan.totalActions).toBe(2);
    const none = await s.api.planSync(id, "leftToRight", options, []);
    expect(none.totalActions).toBe(0);
    const job = await s.api.runSync(plan.token);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, job)).state).toBe("done");
    expect(await names(s.api, `${RIGHT}/ordner-nur-links`)).toEqual(["a.txt"]);
    expect(await names(s.api, RIGHT)).not.toContain("nur-links.txt");
  });

  test("Seiten der Aktionsliste; Lesen verbraucht den Plan nicht", async () => {
    const s = setup();
    const id = await compare(s);
    const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: false });
    const page = await s.api.syncPlanPage(plan.token, 1, 2);
    expect(page).toMatchObject({ total: 5, offset: 1 });
    expect(page.actions.map((a) => a.index)).toEqual([1, 2]);
    expect(page.actions[0]).toEqual(plan.actions[1]);
    expect((await s.api.syncPlanPage(plan.token, 10, 5)).actions).toEqual([]);
    await rejects(s.api.syncPlanPage("nope", 0, 5), "invalidToken");
    await rejects(s.api.syncPlanPage(plan.token, -1, 5), "invalidInput");
    await s.api.runSync(plan.token);
  });

  test("Neueres ersetzen: das geplante Überschreiben fragt über die Konfliktbrücke", async () => {
    for (const [resolution, expected] of [
      ["overwrite", true],
      ["skip", false],
    ] as const) {
      const s = setup();
      const id = await compare(s, `${LEFT}/gemeinsam`, `${RIGHT}/gemeinsam`);
      const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: true });
      expect(plan.actions.map((a) => `${a.kind}:${a.rel}`)).toEqual(["replace:b.txt", "copy:c.txt"]);
      const before = (await s.api.list(`${RIGHT}/gemeinsam`)).entries.find((e) => e.name === "b.txt")!.modified;
      const job = await s.api.runSync(plan.token);
      await s.scheduler.runAll();
      const conflict = s.events.find((e) => e.type === "conflict");
      expect(conflict && conflict.type === "conflict" && conflict.conflict).toMatchObject({
        jobId: job,
        destinationPath: `${RIGHT}/gemeinsam/b.txt`,
      });
      // solange die Frage offen ist, bleibt das Ziel unverändert
      expect((await s.api.list(`${RIGHT}/gemeinsam`)).entries.find((e) => e.name === "b.txt")!.modified).toBe(before);
      await s.api.answerConflict(job, { resolution, applyToAll: false });
      await s.scheduler.runAll();
      expect((await jobOf(s.api, job)).state).toBe("done");
      const after = (await s.api.list(`${RIGHT}/gemeinsam`)).entries.find((e) => e.name === "b.txt")!.modified;
      expect(after === before).toBe(!expected);
      expect(await names(s.api, `${RIGHT}/gemeinsam`)).toContain("c.txt");
    }
  });

  test("Ziel nach der Planung ausgetauscht: Aktion wird übersprungen und gemeldet", async () => {
    const s = setup();
    const id = await compare(s, `${LEFT}/gemeinsam`, `${RIGHT}/gemeinsam`);
    const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: true });
    // b.txt am Ziel durch eine gleich große Datei mit gleicher Zeit ersetzen (anderer Eintrag)
    await s.api.trash([`${RIGHT}/gemeinsam/b.txt`]);
    await s.api.copy([`${LEFT}/gemeinsam/b.txt`], `${RIGHT}/gemeinsam`);
    await s.scheduler.runAll();
    const job = await s.api.runSync(plan.token);
    await s.scheduler.runAll();
    const status = await jobOf(s.api, job);
    expect(status.state).toBe("done");
    expect(status.skipped).toEqual([`${RIGHT}/gemeinsam/b.txt`]);
    expect(status.warnings).toEqual([
      { path: `${RIGHT}/gemeinsam/b.txt`, message: "Übersprungen: Der Eintrag wurde seit der Planung ausgetauscht" },
    ]);
    expect(s.events.some((e) => e.type === "conflict")).toBe(false);
    // der Rest des Plans lief
    expect(await names(s.api, `${RIGHT}/gemeinsam`)).toContain("c.txt");
  });

  test("Zielordner nach der Planung um eine Datei ergänzt: der Ordner wird nicht entfernt", async () => {
    const s = setup();
    const id = await compare(s, `${LEFT}/ordner-nur-links`, `${RIGHT}/ordner-nur-rechts`);
    const plan = await s.api.planSync(id, "mirror", { replaceNewer: false });
    expect(plan.actions.at(-1)).toMatchObject({ kind: "trash", rel: "r.txt" });
    await s.api.copy([`${LEFT}/gleich.txt`], `${RIGHT}/ordner-nur-rechts`);
    await s.scheduler.runAll();
    const job = await s.api.runSync(plan.token);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, job)).state).toBe("done");
    // r.txt war geplant und ging weg; die neue Datei war nicht geplant und bleibt
    expect(await names(s.api, `${RIGHT}/ordner-nur-rechts`)).toContain("gleich.txt");
  });

  test("Archive sind nur Quelle: als Ziel readOnly, als Quelle geht das Entpacken", async () => {
    const s = setup();
    const archive = `archive:${HOME}/Downloads/rezept-paket.tar.gz!/rezepte`;
    const id = await compare(s, archive, `${RIGHT}/gemeinsam`);
    const summary = (await s.api.comparePage(id, 0, 1)).summary;
    expect(summary.left).toBe(archive);
    await rejects(s.api.planSync(id, "rightToLeft", { replaceNewer: false }), "readOnly");
    const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: false });
    expect(plan.actions.some((a) => a.rel === "pizza.txt")).toBe(true);
    const job = await s.api.runSync(plan.token);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, job)).state).toBe("done");
    expect(await names(s.api, `${RIGHT}/gemeinsam`)).toContain("pizza.txt");
    // Archiv als Ziel auf der rechten Seite
    const id2 = await compare(s, `${RIGHT}/gemeinsam`, archive);
    await rejects(s.api.planSync(id2, "leftToRight", { replaceNewer: false }), "readOnly");
    await rejects(s.api.planSync(id2, "mirror", { replaceNewer: false }), "readOnly");
  });

  test("gleiche oder ineinander liegende Ordner werden abgelehnt", async () => {
    const s = setup();
    const same = await compare(s, LEFT, LEFT);
    await rejects(s.api.planSync(same, "leftToRight", { replaceNewer: false }), "invalidTarget");
    const nested = await compare(s, LEFT, `${LEFT}/gemeinsam`);
    await rejects(s.api.planSync(nested, "leftToRight", { replaceNewer: false }), "invalidTarget");
    await rejects(s.api.planSync(nested, "rightToLeft", { replaceNewer: false }), "invalidTarget");
  });

  test("Schließen des Ergebnisses verwirft seine Pläne; alte Pläne werden vergessen", async () => {
    const s = setup();
    const id = await compare(s);
    const options = { replaceNewer: false };
    const plan = await s.api.planSync(id, "leftToRight", options);
    await s.api.compareClose(id);
    await rejects(s.api.runSync(plan.token), "invalidToken");
    await rejects(s.api.planSync(id, "leftToRight", options), "notFound");

    const again = await compare(s);
    const first = await s.api.planSync(again, "leftToRight", options);
    for (let i = 0; i < MAX_SYNC_PLANS; i += 1) await s.api.planSync(again, "leftToRight", options);
    await rejects(s.api.runSync(first.token), "invalidToken");
  });

  test("ein abgebrochener Abgleich bleibt Auftrag mit Abbruch, der Plan ist verbraucht", async () => {
    const s = setup();
    const id = await compare(s);
    const plan = await s.api.planSync(id, "leftToRight", { replaceNewer: false });
    const job = await s.api.runSync(plan.token);
    await s.api.cancelJob(job);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, job)).state).toBe("cancelled");
    await rejects(s.api.runSync(plan.token), "invalidToken");
  });
});

describe("diffFiles", () => {
  const OPTIONS = { ignoreWhitespace: false, ignoreCase: false };

  test("Demo-Dateien: Abschnitte decken beide Dateien lückenlos ab", async () => {
    const s = setup();
    const diff = await s.api.diffFiles(`${TEXT}/vergleich-alt.txt`, `${TEXT}/vergleich-neu.txt`, OPTIONS);
    expect(diff.differences).toBeGreaterThanOrEqual(3);
    expect(diff.hunks.reduce((n, h) => n + h.left.length, 0)).toBe(diff.leftLines);
    expect(diff.hunks.reduce((n, h) => n + h.right.length, 0)).toBe(diff.rightLines);
    let left = 0;
    let right = 0;
    for (const hunk of diff.hunks) {
      expect([hunk.leftStart, hunk.rightStart]).toEqual([left, right]);
      left += hunk.left.length;
      right += hunk.right.length;
    }
    const replaced = diff.hunks.find((h) => h.kind === "replace" && h.left[0] === "- 250 g Mehl");
    expect(replaced?.right).toEqual(["- 300 g Mehl"]);
    expect(diff.hunks.some((h) => h.kind === "insert" && h.right[0] === "- 1 EL Zucker")).toBe(true);
    // letzte Zeile ohne Zeilenumbruch unterscheidet sich von der mit Umbruch
    const last = diff.hunks.at(-1)!;
    expect(last.kind).toBe("replace");
    expect(last.left.at(-1)).toBe("Mit Zucker servieren.");
    expect(last.right.at(-1)).toBe("Mit Zucker servieren");
    const same = await s.api.diffFiles(`${TEXT}/vergleich-alt.txt`, `${TEXT}/vergleich-alt.txt`, OPTIONS);
    expect(same.differences).toBe(0);
    expect(same.hunks).toHaveLength(1);
  });

  test("Binär und zu groß werden mit der Meldung des Kerns abgelehnt", async () => {
    const s = setup();
    const binary = await rejects(s.api.diffFiles(`${TEXT}/vergleich-alt.txt`, `${TEXT}/vergleich-bin.dat`, OPTIONS), "unsupported");
    expect(binary.message).toBe(MSG_DIFF_BINARY);
    const big = await rejects(s.api.diffFiles(`${TEXT}/vergleich-gross.txt`, `${TEXT}/vergleich-alt.txt`, OPTIONS), "unsupported");
    expect(big.message).toBe(MSG_DIFF_TOO_LARGE);
    expect(3 * 1024 * 1024).toBeGreaterThan(DIFF_MAX_BYTES);
  });

  test("Pfade: relativ, Ordner, fehlende Datei", async () => {
    const s = setup();
    await rejects(s.api.diffFiles("rel", `${TEXT}/vergleich-alt.txt`, OPTIONS), "invalidPath");
    await rejects(s.api.diffFiles(TEXT, `${TEXT}/vergleich-alt.txt`, OPTIONS), "io");
    await rejects(s.api.diffFiles(`${TEXT}/fehlt.txt`, `${TEXT}/vergleich-alt.txt`, OPTIONS), "notFound");
  });
});

describe("diffBytes (Zeilenvergleich wie der Kern)", () => {
  const enc = (text: string): Uint8Array => new TextEncoder().encode(text);
  const base = { ignoreWhitespace: false, ignoreCase: false };

  test("leer, gleich, Einfügen, Löschen, Ersetzen", () => {
    expect(diffBytes(enc(""), enc(""), base)).toMatchObject({ hunks: [], differences: 0, leftLines: 0, rightLines: 0 });
    expect(diffBytes(enc("a\nb\n"), enc("a\nb\n"), base).differences).toBe(0);
    expect(diffBytes(enc(""), enc("x\n"), base).hunks).toEqual([
      { kind: "insert", leftStart: 0, left: [], rightStart: 0, right: ["x"] },
    ]);
    expect(diffBytes(enc("x\n"), enc(""), base).hunks[0]).toMatchObject({ kind: "delete", left: ["x"] });
    const hunks = diffBytes(enc("a\nb\nc\n"), enc("a\nB\nc\nd\n"), base).hunks;
    expect(hunks.map((h) => h.kind)).toEqual(["equal", "replace", "equal", "insert"]);
  });

  test("Leerraum und Groß-/Kleinschreibung ignorieren; \\r bleibt in der Zeile", () => {
    expect(diffBytes(enc("a  b\n"), enc("a b\r\n"), { ...base, ignoreWhitespace: true }).differences).toBe(0);
    expect(diffBytes(enc("a  b\n"), enc("a b\n"), base).differences).toBe(1);
    expect(diffBytes(enc("Hallo\n"), enc("hallo\n"), { ...base, ignoreCase: true }).differences).toBe(0);
    expect(diffBytes(enc("a\r\n"), enc("a\r\n"), base).hunks[0]!.left).toEqual(["a\r"]);
  });

  test("Binär (NUL am Anfang), zu viele Zeilen, Unicode", () => {
    expect(() => diffBytes(Uint8Array.from([65, 0, 66]), enc("a"), base)).toThrow(MSG_DIFF_BINARY);
    expect(() => diffBytes(enc("a\n".repeat(50_001)), enc("a"), base)).toThrow(MSG_DIFF_TOO_LARGE);
    const uni = diffBytes(enc("Grüße\nÄÖÜ\n"), enc("Grüße\näöü\n"), base);
    expect(uni.hunks[1]).toMatchObject({ kind: "replace", left: ["ÄÖÜ"], right: ["äöü"] });
  });
});
