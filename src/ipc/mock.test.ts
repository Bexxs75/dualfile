import { describe, expect, test } from "vitest";
import { CoreError, type DualfileApi } from "./api";
import { createMockApi, MOCK_SEARCH_LIMIT, type MockScheduler } from "./mock";
import type { ConflictInfo, CoreEvent, JobStatus, Settings } from "./types";

const HOME = "/home/demo";
const DOCS = `${HOME}/Dokumente`;
const DOWNLOADS = `${HOME}/Downloads`;

/** Von Hand gesteuerter Zeitgeber: Schritte laufen erst bei `step`/`runAll`. */
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
  /** Lässt wartende Fortsetzungen (Microtasks) ablaufen. */
  async settle(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  /** Führt genau einen Zeitgeber-Schritt aus. */
  async step(): Promise<boolean> {
    const first = this.timers.entries().next();
    if (first.done) return false;
    this.timers.delete(first.value[0]);
    first.value[1]();
    await this.settle();
    return true;
  }
  /** Läuft, bis nichts mehr geplant ist (auch wenn der Auftrag auf eine Antwort wartet). */
  async runAll(): Promise<void> {
    await this.settle();
    while (await this.step()) {
      /* weiter */
    }
  }
}

function setup(): { api: DualfileApi; scheduler: ManualScheduler; events: CoreEvent[] } {
  const scheduler = new ManualScheduler();
  const api = createMockApi({ scheduler, now: () => Date.UTC(2026, 10, 12, 12, 0) });
  const events: CoreEvent[] = [];
  api.onEvent((event) => events.push(event));
  return { api, scheduler, events };
}

const names = async (api: DualfileApi, path: string): Promise<string[]> =>
  (await api.list(path)).entries.map((entry) => entry.name);
const sizeOf = async (api: DualfileApi, path: string, name: string): Promise<number | undefined> =>
  (await api.list(path)).entries.find((entry) => entry.name === name)?.size;
const jobOf = async (api: DualfileApi, id: number): Promise<JobStatus> => {
  const job = (await api.jobs()).find((candidate) => candidate.id === id);
  if (!job) throw new Error(`Auftrag ${id} fehlt`);
  return job;
};
const conflicts = (events: CoreEvent[]): ConflictInfo[] =>
  events.flatMap((event) => (event.type === "conflict" ? [event.conflict] : []));
const changedPaths = (events: CoreEvent[]): string[] =>
  events.flatMap((event) => (event.type === "dirChanged" ? [event.path] : []));

/** `Downloads/notizen.txt` (21 Bytes) steht der Quelle `Dokumente/notizen.txt` (1200 Bytes) im Weg. */
async function makeConflictTarget(api: DualfileApi): Promise<void> {
  await api.rename(`${DOWNLOADS}/Datei2.txt`, "notizen.txt");
}

describe("Beispieldaten und list", () => {
  test("list liefert Home mit den vorgesehenen Beispielen", async () => {
    const { api } = setup();
    const byName = new Map((await api.list(HOME)).entries.map((entry) => [entry.name, entry]));
    for (const name of ["Dokumente", "Bilder", "Downloads", "Projekte"]) expect(byName.get(name)?.kind).toBe("dir");
    expect(byName.get(".bashrc")?.hidden).toBe(true);
    expect(byName.get("Dokumente")?.hidden).toBe(false);
    expect(byName.get("große-datei.bin")?.size).toBeGreaterThan(10_000_000_000);
    expect(byName.get("Projekte-Link")?.kind).toBe("symlink");
    expect(byName.get("Projekte-Link")?.brokenLink).toBe(false);
    expect(byName.get("Defekter-Link")?.brokenLink).toBe(true);
    expect(await names(api, DOCS)).toContain("Änderungen.txt");
    expect(await names(api, DOWNLOADS)).toEqual(expect.arrayContaining(["Datei1.txt", "Datei2.txt", "Datei10.txt"]));
  });

  test("Tilde und absoluter Pfad zeigen auf denselben Ordner, homeDir stimmt", async () => {
    const { api } = setup();
    expect(await api.homeDir()).toBe(HOME);
    const tilde = await api.list("~");
    expect(tilde.path).toBe(HOME);
    expect(tilde.entries).toEqual((await api.list(HOME)).entries);
  });

  test("unbekannter Pfad wirft CoreError mit Meldung „nicht gefunden“", async () => {
    const { api } = setup();
    const error = await api.list("/gibt/es/nicht").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).message).toMatch(/nicht gefunden/i);
    expect((error as CoreError).kind).toBe("notFound");
  });

  test("list auf eine Datei und auf einen nicht lesbaren Ordner schlägt fehl", async () => {
    const { api } = setup();
    await expect(api.list(`${DOCS}/notizen.txt`)).rejects.toBeInstanceOf(CoreError);
    await expect(api.list(`${HOME}/Projekte/gesperrt`)).rejects.toThrow(/verweigert/);
  });

  test("eigener Home-Pfad wird angelegt", async () => {
    const api = createMockApi({ home: "/Users/test" });
    expect(await api.homeDir()).toBe("/Users/test");
    expect(await names(api, "/Users/test")).toContain("Dokumente");
  });
});

describe("Kopieren und Verschieben", () => {
  test("copy kopiert Ordner rekursiv, Quelle bleibt", async () => {
    const { api, scheduler } = setup();
    const id = await api.copy([`${HOME}/Projekte/dualfile`], DOWNLOADS);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    expect(await names(api, `${DOWNLOADS}/dualfile`)).toEqual(expect.arrayContaining(["src", "README.md"]));
    expect(await names(api, `${DOWNLOADS}/dualfile/src`)).toEqual(["main.ts"]);
    expect(await names(api, `${HOME}/Projekte/dualfile`)).toContain("README.md");
  });

  test("die Kopie ist vom Original unabhängig", async () => {
    const { api, scheduler } = setup();
    await api.copy([`${HOME}/Projekte/dualfile`], DOWNLOADS);
    await scheduler.runAll();
    await api.rename(`${DOWNLOADS}/dualfile/README.md`, "LIESMICH.md");
    expect(await names(api, `${HOME}/Projekte/dualfile`)).toContain("README.md");
  });

  test("Fortschritt wächst monoton bis zur Gesamtgröße", async () => {
    const { api, scheduler, events } = setup();
    const id = await api.copy([`${DOWNLOADS}/setup.AppImage`], DOCS);
    await scheduler.runAll();
    const reports = events.flatMap((e) => (e.type === "job" && e.status.id === id ? [e.status] : []));
    const bytes = reports.map((status) => status.bytesDone);
    expect(bytes.length).toBeGreaterThan(2);
    expect(bytes).toEqual([...bytes].sort((a, b) => a - b));
    expect(reports[0]?.state).toBe("queued");
    const last = reports.at(-1);
    expect(last?.state).toBe("done");
    expect(last?.bytesDone).toBe(129_000_000);
    expect(last?.bytesTotal).toBe(129_000_000);
    expect(last?.filesDone).toBe(1);
  });

  test("Aufträge laufen nur über den Zeitgeber, nicht sofort", async () => {
    const { api, scheduler } = setup();
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.settle();
    expect((await jobOf(api, id)).state).toBe("queued");
    expect(await names(api, DOWNLOADS)).not.toContain("notizen.txt");
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
  });

  test("Konflikt wartet auf die Antwort und überschreibt vorher nie", async () => {
    const { api, scheduler, events } = setup();
    await makeConflictTarget(api);
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    const asked = conflicts(events);
    expect(asked).toHaveLength(1);
    expect(asked[0]?.jobId).toBe(id);
    expect(asked[0]?.sourcePath).toBe(`${DOCS}/notizen.txt`);
    expect(asked[0]?.destinationPath).toBe(`${DOWNLOADS}/notizen.txt`);
    expect(asked[0]?.source.size).toBe(1_200);
    expect(asked[0]?.destination.size).toBe(21);
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect((await jobOf(api, id)).state).not.toBe("done");
    await scheduler.runAll();
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21); // auch nach weiterer Zeit unverändert
  });

  test("Antwort overwrite ersetzt das Ziel", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "overwrite", applyToAll: false });
    await scheduler.runAll();
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(1_200);
    expect((await names(api, DOWNLOADS)).filter((name) => name === "notizen.txt")).toHaveLength(1);
    expect((await jobOf(api, id)).state).toBe("done");
  });

  test("Antwort skip lässt das Ziel unberührt und vermerkt die Quelle", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "skip", applyToAll: false });
    await scheduler.runAll();
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21);
    const job = await jobOf(api, id);
    expect(job.skipped).toEqual([`${DOCS}/notizen.txt`]);
    expect(job.state).toBe("done");
  });

  test("Antwort keepBoth legt „name (2).ext“ an und lässt das Original", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "keepBoth", applyToAll: false });
    await scheduler.runAll();
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect(await sizeOf(api, DOWNLOADS, "notizen (2).txt")).toBe(1_200);
  });

  test("keepBoth zählt weiter, wenn „(2)“ schon vergeben ist", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    await api.rename(`${DOWNLOADS}/Datei1.txt`, "notizen (2).txt");
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "keepBoth", applyToAll: false });
    await scheduler.runAll();
    expect(await sizeOf(api, DOWNLOADS, "notizen (3).txt")).toBe(1_200);
    expect(await sizeOf(api, DOWNLOADS, "notizen (2).txt")).toBe(12);
  });

  test("Antwort cancel bricht den Auftrag ab und ändert nichts", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.copy([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "cancel", applyToAll: false });
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("cancelled");
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect(await names(api, DOWNLOADS)).not.toContain("Änderungen.txt");
  });

  test("„für alle“ beantwortet weitere Konflikte ohne erneute Frage", async () => {
    const { api, scheduler, events } = setup();
    await makeConflictTarget(api);
    await api.rename(`${DOWNLOADS}/Datei1.txt`, "Änderungen.txt");
    const id = await api.copy([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "skip", applyToAll: true });
    await scheduler.runAll();
    expect(conflicts(events)).toHaveLength(1);
    const job = await jobOf(api, id);
    expect(job.skipped).toHaveLength(2);
    expect(job.state).toBe("done");
    expect(await sizeOf(api, DOWNLOADS, "Änderungen.txt")).toBe(12);
  });

  test("ohne „für alle“ wird jeder Konflikt einzeln gefragt", async () => {
    const { api, scheduler, events } = setup();
    await makeConflictTarget(api);
    await api.rename(`${DOWNLOADS}/Datei1.txt`, "Änderungen.txt");
    const id = await api.copy([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "skip", applyToAll: false });
    await scheduler.runAll();
    expect(conflicts(events)).toHaveLength(2);
  });

  test("answerConflict ohne wartenden Konflikt wirft einen Fehler", async () => {
    const { api } = setup();
    await expect(api.answerConflict(99, { resolution: "skip", applyToAll: false })).rejects.toBeInstanceOf(CoreError);
  });

  test("Ordner gleichen Namens werden zusammengeführt, Dateikonflikte darin fragen einzeln", async () => {
    const { api, scheduler, events } = setup();
    await api.makeDir(DOWNLOADS, "Projekte");
    await api.copy([`${HOME}/Projekte/dualfile`], `${DOWNLOADS}/Projekte`);
    await scheduler.runAll();
    const id = await api.copy([`${HOME}/Projekte`], DOWNLOADS);
    await scheduler.runAll();
    expect(conflicts(events).at(-1)?.jobId).toBe(id); // main.ts oder README.md im Zielordner
    await api.answerConflict(id, { resolution: "skip", applyToAll: true });
    await scheduler.runAll();
    expect(await names(api, `${DOWNLOADS}/Projekte`)).toEqual(expect.arrayContaining(["dualfile", "3mf-katalog"]));
  });

  test("Fehler bei einem Eintrag stoppt den Auftrag nicht, er steht in failed", async () => {
    const { api, scheduler } = setup();
    const id = await api.copy([`${HOME}/Projekte/gesperrt`, `${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    const job = await jobOf(api, id);
    expect(job.state).toBe("failed");
    expect(job.failed).toEqual([{ path: `${HOME}/Projekte/gesperrt`, message: "Zugriff verweigert" }]);
    expect(await names(api, DOWNLOADS)).toContain("notizen.txt");
    expect(await names(api, DOWNLOADS)).not.toContain("gesperrt");
  });

  test("Ziel innerhalb der Quelle und unbekannte Quelle werden abgelehnt", async () => {
    const { api } = setup();
    await expect(api.copy([`${HOME}/Projekte`], `${HOME}/Projekte/dualfile`)).rejects.toBeInstanceOf(CoreError);
    await expect(api.move([`${HOME}/Projekte`], `${HOME}/Projekte`)).rejects.toBeInstanceOf(CoreError);
    await expect(api.copy([`${HOME}/fehlt.txt`], DOWNLOADS)).rejects.toThrow(/nicht gefunden/i);
    await expect(api.copy([`${DOCS}/notizen.txt`], `${DOCS}/notizen.txt`)).rejects.toBeInstanceOf(CoreError);
  });

  test("Kopieren in den eigenen Ordner meldet „dieselbe Datei“ statt zu überschreiben", async () => {
    const { api, scheduler } = setup();
    const id = await api.copy([`${DOCS}/notizen.txt`], DOCS);
    await scheduler.runAll();
    const job = await jobOf(api, id);
    expect(job.failed).toHaveLength(1);
    expect(job.failed[0]?.message).toMatch(/dieselbe Datei/);
    expect(await sizeOf(api, DOCS, "notizen.txt")).toBe(1_200);
  });

  test("move verschiebt Datei und Ordner, die Quelle verschwindet", async () => {
    const { api, scheduler } = setup();
    const id = await api.move([`${DOCS}/notizen.txt`, `${HOME}/Projekte/dualfile`], DOWNLOADS);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    expect(await names(api, DOCS)).not.toContain("notizen.txt");
    expect(await names(api, `${HOME}/Projekte`)).not.toContain("dualfile");
    expect(await names(api, DOWNLOADS)).toEqual(expect.arrayContaining(["notizen.txt", "dualfile"]));
    expect(await names(api, `${DOWNLOADS}/dualfile/src`)).toEqual(["main.ts"]);
  });

  test("move mit skip lässt die übersprungene Quelle stehen", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.move([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "skip", applyToAll: false });
    await scheduler.runAll();
    expect(await names(api, DOCS)).toContain("notizen.txt");
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21);
  });

  test("move mit overwrite ersetzt das Ziel und entfernt die Quelle", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.move([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.answerConflict(id, { resolution: "overwrite", applyToAll: false });
    await scheduler.runAll();
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(1_200);
    expect(await names(api, DOCS)).not.toContain("notizen.txt");
  });
});

describe("Abbruch", () => {
  test("cancelJob vor dem ersten Schritt: nichts wird kopiert", async () => {
    const { api, scheduler } = setup();
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await api.cancelJob(id);
    expect((await jobOf(api, id)).state).toBe("cancelled");
    await scheduler.runAll();
    expect(await names(api, DOWNLOADS)).not.toContain("notizen.txt");
    expect((await jobOf(api, id)).state).toBe("cancelled");
  });

  test("cancelJob mitten in einer großen Datei stoppt weitere Blöcke", async () => {
    const { api, scheduler, events } = setup();
    const target = `${HOME}/Musik`;
    const id = await api.copy([`${DOWNLOADS}/setup.AppImage`, `${DOCS}/notizen.txt`], target);
    await scheduler.step(); // Start des Auftrags
    await scheduler.step(); // erster Block
    const before = (await jobOf(api, id)).bytesDone;
    expect(before).toBeGreaterThan(0);
    await api.cancelJob(id);
    await scheduler.runAll();
    const job = await jobOf(api, id);
    expect(job.state).toBe("cancelled");
    expect(job.bytesDone).toBe(before);
    expect(await names(api, target)).toEqual([]);
    expect(events.some((e) => e.type === "job" && e.status.id === id && e.status.state === "done")).toBe(false);
  });

  test("cancelJob bricht auch einen auf Antwort wartenden Auftrag ab", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.cancelJob(id);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("cancelled");
    expect(await sizeOf(api, DOWNLOADS, "notizen.txt")).toBe(21);
  });

  test("cancelJob mit unbekannter Nummer wirft, bei beendetem Auftrag ist es wirkungslos", async () => {
    const { api, scheduler } = setup();
    await expect(api.cancelJob(42)).rejects.toBeInstanceOf(CoreError);
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.cancelJob(id);
    expect((await jobOf(api, id)).state).toBe("done");
  });
});

describe("Papierkorb", () => {
  test("trash entfernt den Eintrag, restoreFromTrash stellt ihn über den Schlüssel wieder her", async () => {
    const { api } = setup();
    const id = await api.trash([`${DOCS}/notizen.txt`]);
    expect(await names(api, DOCS)).not.toContain("notizen.txt");
    expect((await jobOf(api, id)).state).toBe("done");
    await api.restoreFromTrash([`trash://${id}/notizen.txt`]);
    expect(await sizeOf(api, DOCS, "notizen.txt")).toBe(1_200);
  });

  test("restoreFromTrash akzeptiert auch den ursprünglichen Pfad, Ordner kehren vollständig zurück", async () => {
    const { api } = setup();
    await api.trash([`${HOME}/Projekte/dualfile`]);
    await api.restoreFromTrash([`${HOME}/Projekte/dualfile`]);
    expect(await names(api, `${HOME}/Projekte/dualfile/src`)).toEqual(["main.ts"]);
  });

  test("Wiederherstellen scheitert bei unbekanntem Eintrag", async () => {
    const { api } = setup();
    await expect(api.restoreFromTrash(["trash://9/nix"])).rejects.toThrow(/Papierkorb/);
  });

  test("Wiederherstellen scheitert bei vergebenem Namen und lässt den Eintrag im Papierkorb", async () => {
    const { api } = setup();
    const id = await api.trash([`${DOCS}/notizen.txt`]);
    await api.rename(`${DOCS}/Übersicht.ods`, "notizen.txt");
    await expect(api.restoreFromTrash([`trash://${id}/notizen.txt`])).rejects.toThrow(/vorhanden/);
    await api.rename(`${DOCS}/notizen.txt`, "frei.ods");
    await api.restoreFromTrash([`trash://${id}/notizen.txt`]); // jetzt möglich
    expect(await sizeOf(api, DOCS, "notizen.txt")).toBe(1_200);
  });

  test("Wiederherstellen scheitert, wenn der ursprüngliche Ordner fehlt", async () => {
    const { api } = setup();
    const id = await api.trash([`${DOCS}/notizen.txt`]);
    await api.trash([DOCS]);
    await expect(api.restoreFromTrash([`trash://${id}/notizen.txt`])).rejects.toThrow(/Ordner fehlt/);
  });

  test("zwei Einträge gleichen Namens in einem Auftrag bekommen verschiedene Schlüssel", async () => {
    const { api } = setup();
    await api.rename(`${DOCS}/notizen.txt`, "a.txt");
    await api.rename(`${DOWNLOADS}/Datei1.txt`, "a.txt");
    const id = await api.trash([`${DOCS}/a.txt`, `${DOWNLOADS}/a.txt`]);
    await api.restoreFromTrash([`trash://${id}/a.txt`, `trash://${id}/a.txt (2)`]);
    expect(await names(api, DOCS)).toContain("a.txt");
    expect(await names(api, DOWNLOADS)).toContain("a.txt");
  });

  test("trash lehnt unbekannte Pfade und die Wurzel ab, ohne etwas zu entfernen", async () => {
    const { api } = setup();
    await expect(api.trash([`${DOCS}/notizen.txt`, `${DOCS}/fehlt`])).rejects.toBeInstanceOf(CoreError);
    expect(await names(api, DOCS)).toContain("notizen.txt");
    await expect(api.trash(["/"])).rejects.toBeInstanceOf(CoreError);
  });
});

describe("Löschen mit Token", () => {
  test("planDelete zählt Dateien, Ordner, Links und Bytes und nennt die ersten Namen", async () => {
    const { api } = setup();
    const plan = await api.planDelete([`${HOME}/Projekte/dualfile`, `${HOME}/Projekte-Link`]);
    expect(plan).toMatchObject({ files: 2, dirs: 2, links: 1, bytes: 14_500, incomplete: false });
    expect(plan.firstNames).toEqual(["dualfile", "Projekte-Link"]);
    expect(plan.token).not.toBe("");
  });

  test("falsches Token: Fehler und nichts gelöscht, der echte Plan bleibt nutzbar", async () => {
    const { api } = setup();
    const plan = await api.planDelete([`${DOCS}/notizen.txt`]);
    const error = await api.deletePermanent(plan, "falsch").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).kind).toBe("invalidToken");
    expect(await names(api, DOCS)).toContain("notizen.txt");
    await api.deletePermanent(plan, plan.token);
    expect(await names(api, DOCS)).not.toContain("notizen.txt");
  });

  test("erfundener Plan mit unbekanntem Token löscht nichts", async () => {
    const { api } = setup();
    const before = await names(api, DOCS);
    const forged = { token: "x", files: 1, dirs: 0, links: 0, bytes: 1, firstNames: [], incomplete: false };
    await expect(api.deletePermanent(forged, "x")).rejects.toBeInstanceOf(CoreError);
    expect(await names(api, DOCS)).toEqual(before);
  });

  test("ein Plan gilt nur einmal", async () => {
    const { api } = setup();
    const plan = await api.planDelete([`${DOCS}/notizen.txt`]);
    await api.deletePermanent(plan, plan.token);
    await expect(api.deletePermanent(plan, plan.token)).rejects.toBeInstanceOf(CoreError);
  });

  test("hat sich der Umfang seit der Planung geändert, wird nichts gelöscht", async () => {
    const { api } = setup();
    const plan = await api.planDelete([`${HOME}/Projekte/dualfile`]);
    await api.makeDir(`${HOME}/Projekte/dualfile`, "neu");
    const error = await api.deletePermanent(plan, plan.token).catch((e: unknown) => e);
    expect((error as CoreError).kind).toBe("planChanged");
    expect(await names(api, `${HOME}/Projekte`)).toContain("dualfile");
  });

  test("Löschen entfernt Ordner samt Inhalt; ein Link wird entfernt, sein Ziel bleibt", async () => {
    const { api } = setup();
    const plan = await api.planDelete([`${HOME}/Projekte-Link`, `${HOME}/Projekte/dualfile`]);
    const id = await api.deletePermanent(plan, plan.token);
    expect((await jobOf(api, id)).state).toBe("done");
    expect(await names(api, HOME)).not.toContain("Projekte-Link");
    expect(await names(api, `${HOME}/Projekte`)).toContain("3mf-katalog");
    expect(await names(api, `${HOME}/Projekte`)).not.toContain("dualfile");
  });

  test("planDelete lehnt unbekannte Pfade und die Wurzel ab", async () => {
    const { api } = setup();
    await expect(api.planDelete([`${DOCS}/fehlt`])).rejects.toBeInstanceOf(CoreError);
    await expect(api.planDelete(["/"])).rejects.toBeInstanceOf(CoreError);
  });
});

describe("Umbenennen und Ordner anlegen", () => {
  test("rename ändert den Namen und liefert alten und neuen Pfad", async () => {
    const { api } = setup();
    expect(await api.rename(`${DOCS}/notizen.txt`, "ideen.txt")).toEqual({
      oldPath: `${DOCS}/notizen.txt`,
      newPath: `${DOCS}/ideen.txt`,
    });
    expect(await names(api, DOCS)).toContain("ideen.txt");
    expect(await names(api, DOCS)).not.toContain("notizen.txt");
  });

  test("rename setzt das Versteckt-Kennzeichen nach dem Punkt am Namensanfang", async () => {
    const { api } = setup();
    await api.rename(`${DOCS}/notizen.txt`, ".versteckt");
    expect((await api.list(DOCS)).entries.find((e) => e.name === ".versteckt")?.hidden).toBe(true);
  });

  test.each([
    ["leer", ""],
    ["Punkt", "."],
    ["zwei Punkte", ".."],
    ["Schrägstrich", "a/b"],
    ["zu lang (256 Bytes)", "ä".repeat(128)],
  ])("rename lehnt ungültigen Namen ab: %s", async (_label, name) => {
    const { api } = setup();
    const error = await api.rename(`${DOCS}/notizen.txt`, name).catch((e: unknown) => e);
    expect((error as CoreError).kind).toBe("invalidName");
    expect(await names(api, DOCS)).toContain("notizen.txt");
  });

  test("rename akzeptiert genau 255 Bytes", async () => {
    const { api } = setup();
    const name = "a".repeat(255);
    await api.rename(`${DOCS}/notizen.txt`, name);
    expect(await names(api, DOCS)).toContain(name);
  });

  test("rename auf einen bestehenden Namen schlägt fehl und lässt alles unverändert", async () => {
    const { api } = setup();
    const error = await api.rename(`${DOCS}/notizen.txt`, "Änderungen.txt").catch((e: unknown) => e);
    expect((error as CoreError).kind).toBe("exists");
    expect(await sizeOf(api, DOCS, "notizen.txt")).toBe(1_200);
    expect(await sizeOf(api, DOCS, "Änderungen.txt")).toBe(4_000);
  });

  test("rename: unbekannter Pfad und Wurzel werden abgelehnt", async () => {
    const { api } = setup();
    await expect(api.rename(`${DOCS}/fehlt`, "x")).rejects.toThrow(/nicht gefunden/i);
    await expect(api.rename("/", "x")).rejects.toBeInstanceOf(CoreError);
  });

  test("makeDir legt einen leeren Ordner an und liefert den Pfad", async () => {
    const { api } = setup();
    expect(await api.makeDir(DOCS, "Neu")).toBe(`${DOCS}/Neu`);
    expect((await api.list(`${DOCS}/Neu`)).entries).toEqual([]);
    expect((await api.list(DOCS)).entries.find((e) => e.name === "Neu")?.modified).toBe(Date.UTC(2026, 10, 12, 12, 0));
  });

  test("makeDir prüft Namen, Kollisionen und Elternordner", async () => {
    const { api } = setup();
    await expect(api.makeDir(DOCS, "")).rejects.toMatchObject({ kind: "invalidName" });
    await expect(api.makeDir(DOCS, "a/b")).rejects.toMatchObject({ kind: "invalidName" });
    await expect(api.makeDir(DOCS, "notizen.txt")).rejects.toMatchObject({ kind: "exists" });
    await expect(api.makeDir(`${DOCS}/fehlt`, "x")).rejects.toMatchObject({ kind: "notFound" });
    await expect(api.makeDir(`${DOCS}/notizen.txt`, "x")).rejects.toBeInstanceOf(CoreError);
  });
});

describe("Ereignisse", () => {
  test("makeDir, rename und trash senden dirChanged für den betroffenen Ordner", async () => {
    const { api, events } = setup();
    await api.makeDir(DOCS, "Neu");
    await api.rename(`${DOCS}/notizen.txt`, "x.txt");
    await api.trash([`${DOCS}/x.txt`]);
    expect(changedPaths(events)).toEqual([DOCS, DOCS, DOCS]);
  });

  test("copy meldet das Zielverzeichnis, move zusätzlich das Quellverzeichnis", async () => {
    const { api, scheduler, events } = setup();
    await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    expect(changedPaths(events)).toEqual([DOWNLOADS]);
    events.length = 0;
    await api.move([`${DOCS}/Änderungen.txt`], DOWNLOADS);
    await scheduler.runAll();
    expect(changedPaths(events)).toEqual(expect.arrayContaining([DOWNLOADS, DOCS]));
  });

  test("deletePermanent und restoreFromTrash senden dirChanged", async () => {
    const { api, events } = setup();
    const plan = await api.planDelete([`${DOCS}/notizen.txt`]);
    await api.deletePermanent(plan, plan.token);
    expect(changedPaths(events)).toEqual([DOCS]);
    const id = await api.trash([`${DOCS}/Änderungen.txt`]);
    events.length = 0;
    await api.restoreFromTrash([`trash://${id}/Änderungen.txt`]);
    expect(events).toEqual([{ type: "dirChanged", path: DOCS }]);
  });

  test("Abmelden beendet die Zustellung", async () => {
    const { api } = setup();
    const received: CoreEvent[] = [];
    const stop = api.onEvent((event) => received.push(event));
    await api.makeDir(DOCS, "eins");
    stop();
    await api.makeDir(DOCS, "zwei");
    expect(received).toHaveLength(1);
  });

  test("der Standard-Zeitgeber arbeitet mit echter Zeit", async () => {
    const api = createMockApi();
    const id = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    const deadline = Date.now() + 2_000;
    while ((await jobOf(api, id)).state !== "done" && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    expect((await jobOf(api, id)).state).toBe("done");
    expect(await names(api, DOWNLOADS)).toContain("notizen.txt");
  });
});

describe("Einstellungen", () => {
  const settings: Settings = {
    version: 1, verifyPolicy: "auto",
    theme: "papier",
    accent: "#2f6f6a",
    density: "compact",
    fontSize: 15,
    language: "en",
    bookmarks: [{ name: "Projekte", path: `${HOME}/Projekte`, color: "#7bc08a", order: 0 }],
    session: { windows: [{ tabs: [{ path: HOME, sort: "name", filter: "" }], activeTab: 0 }], activeWindow: 0 },
  };

  test("ohne gespeicherte Einstellungen kommen Standardwerte mit Ergebnis „fresh“", async () => {
    const { api } = setup();
    const loaded = await api.loadSettings();
    expect(loaded.outcome).toEqual({ kind: "fresh" });
    expect(loaded.settings.theme).toBe("schiefer");
    expect(loaded.settings.language).toBe("auto");
  });

  test("saveSettings und loadSettings: Roundtrip im Speicher", async () => {
    const { api } = setup();
    await api.saveSettings(settings);
    const loaded = await api.loadSettings();
    expect(loaded.settings).toEqual(settings);
    expect(loaded.outcome).toEqual({ kind: "loaded" });
  });

  test("gespeicherte Werte sind von späteren Änderungen am Objekt entkoppelt", async () => {
    const { api } = setup();
    const mutable = structuredClone(settings);
    await api.saveSettings(mutable);
    mutable.theme = "werkstatt";
    const loaded = await api.loadSettings();
    loaded.settings.accent = "#000000";
    expect((await api.loadSettings()).settings).toEqual(settings);
  });

  test("initialSettings werden beim ersten Laden geliefert", async () => {
    const api = createMockApi({ initialSettings: settings });
    expect((await api.loadSettings()).settings).toEqual(settings);
  });
});

describe("Lister: fileInfo, readBlock, findInFile", () => {
  const NOTES = `${DOCS}/notizen.txt`; // 1200 Bytes Text
  const text = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);
  const needle = (value: string): Uint8Array => new TextEncoder().encode(value);

  test("fileInfo liefert Größe, Art, Zeit und Typ", async () => {
    const { api } = setup();
    expect(await api.fileInfo(NOTES)).toMatchObject({ size: 1_200, kind: "text", mimeGuess: "text/plain" });
    expect(await api.fileInfo(`${HOME}/Bilder/familie.png`)).toMatchObject({ kind: "image", mimeGuess: "image/png" });
    expect(await api.fileInfo(`${HOME}/Bilder/urlaub-2026.jpg`)).toMatchObject({ kind: "image", mimeGuess: "image/jpeg" });
    expect(await api.fileInfo(`${HOME}/Downloads/setup.AppImage`)).toMatchObject({ kind: "binary" });
    expect(typeof (await api.fileInfo(NOTES)).modified).toBe("number");
  });

  test("fileInfo und readBlock folgen Links und lehnen Ordner und Fehlendes ab", async () => {
    const { api } = setup();
    await expect(api.fileInfo(DOCS)).rejects.toBeInstanceOf(CoreError);
    await expect(api.readBlock(`${HOME}/Projekte-Link`, 0, 1)).rejects.toMatchObject({ kind: "io" }); // Link auf Ordner
    await expect(api.readBlock(`${HOME}/Defekter-Link`, 0, 1)).rejects.toMatchObject({ kind: "notFound" });
    await expect(api.fileInfo(`${DOCS}/fehlt.txt`)).rejects.toMatchObject({ kind: "notFound" });
    await expect(api.readBlock("relativ", 0, 1)).rejects.toMatchObject({ kind: "invalidPath" });
  });

  test("readBlock: Anfang, Mitte, Ende, über das Ende, Länge 0", async () => {
    const { api } = setup();
    const all = await api.readBlock(NOTES, 0, 5_000);
    expect(all.length).toBe(1_200);
    expect(text(all.subarray(0, 14))).toBe("Zeile 00000000");
    expect(text(await api.readBlock(NOTES, 64, 14))).toBe("Zeile 00000001");
    expect(await api.readBlock(NOTES, 100, 50)).toEqual(all.subarray(100, 150));
    expect((await api.readBlock(NOTES, 1_190, 50)).length).toBe(10);
    expect((await api.readBlock(NOTES, 1_200, 10)).length).toBe(0);
    expect((await api.readBlock(NOTES, 0, 0)).length).toBe(0);
  });

  test("readBlock kürzt auf 1 MiB, auch bei der riesigen Datei, und liefert stabile Bytes", async () => {
    const { api } = setup();
    const big = `${HOME}/große-datei.bin`;
    const block = await api.readBlock(big, 5_000_000_000, 10_000_000);
    expect(block.length).toBe(1024 * 1024);
    expect(await api.readBlock(big, 5_000_000_000, 16)).toEqual(block.subarray(0, 16));
  });

  test("Inhalte folgen der Dateigröße und enthalten Signaturen", async () => {
    const { api } = setup();
    const png = await api.readBlock(`${HOME}/Bilder/familie.png`, 0, 8);
    expect([...png]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const { size } = await api.fileInfo(`${DOCS}/Änderungen.txt`);
    expect((await api.readBlock(`${DOCS}/Änderungen.txt`, 0, 1_000_000)).length).toBe(size);
  });

  test("findInFile: Treffer vorwärts, ab Startpunkt, nicht vorhanden", async () => {
    const { api } = setup();
    expect(await api.findInFile(NOTES, needle("Zeile 00000003"), 0, "forward", false)).toEqual({ kind: "found", offset: 192 });
    expect(await api.findInFile(NOTES, needle("Zeile 00000003"), 193, "forward", false)).toEqual({ kind: "notFound" });
    expect(await api.findInFile(NOTES, needle("gibt es nicht"), 0, "forward", false)).toEqual({ kind: "notFound" });
  });

  test("findInFile: Groß-/Kleinschreibung", async () => {
    const { api } = setup();
    expect(await api.findInFile(NOTES, needle("beispieltext"), 0, "forward", false)).toEqual({ kind: "notFound" });
    expect(await api.findInFile(NOTES, needle("beispieltext"), 0, "forward", true)).toEqual({ kind: "found", offset: 16 });
  });

  test("findInFile rückwärts: letzter Treffer vor dem Startpunkt", async () => {
    const { api } = setup();
    expect(await api.findInFile(NOTES, needle("Zeile"), 1_200, "backward", false)).toEqual({ kind: "found", offset: 1_152 });
    expect(await api.findInFile(NOTES, needle("Zeile"), 1_152, "backward", false)).toEqual({ kind: "found", offset: 1_088 });
    expect(await api.findInFile(NOTES, needle("Zeile"), 0, "backward", false)).toEqual({ kind: "notFound" });
    expect(await api.findInFile(NOTES, needle("Zeile"), 1e12, "backward", false)).toEqual({ kind: "found", offset: 1_152 });
  });

  test("findInFile findet einen Treffer über die Blockgrenze (1 MiB)", async () => {
    const { api } = setup();
    const big = `${HOME}/große-datei.bin`;
    const border = 1024 * 1024;
    const window = await api.readBlock(big, border - 3, 6); // 3 Bytes vor, 3 Bytes nach der Grenze
    const found = await api.findInFile(big, window, border - 100, "forward", false);
    expect(found).toEqual({ kind: "found", offset: border - 3 });
    const startFromBlock = await api.findInFile(big, window, 0, "forward", false);
    expect(startFromBlock.kind).toBe("found"); // ein früherer gleicher Bytefolgen-Treffer wäre ebenfalls gültig
  });

  test("findInFile: Streckenlimit mit Fortsetzungs-Offset", async () => {
    const { api } = setup();
    const big = `${HOME}/große-datei.bin`;
    const first = await api.findInFile(big, needle("unwahrscheinlich-9999"), 0, "forward", false);
    expect(first).toEqual({ kind: "continue", next: MOCK_SEARCH_LIMIT });
    const second = await api.findInFile(big, needle("unwahrscheinlich-9999"), MOCK_SEARCH_LIMIT, "forward", false);
    expect(second).toEqual({ kind: "continue", next: 2 * MOCK_SEARCH_LIMIT });
    const backwards = await api.findInFile(big, needle("unwahrscheinlich-9999"), 3 * MOCK_SEARCH_LIMIT, "backward", false);
    expect(backwards).toEqual({ kind: "continue", next: 2 * MOCK_SEARCH_LIMIT });
  });

  test("cancelSearch bricht die laufende Suche mit der Art „cancelled“ ab", async () => {
    const { api } = setup();
    const running = api.findInFile(`${HOME}/große-datei.bin`, needle("unwahrscheinlich-9999"), 0, "forward", false);
    const outcome = running.catch((error: unknown) => error);
    await api.cancelSearch();
    expect(await outcome).toMatchObject({ kind: "cancelled" });
    await expect(api.cancelSearch()).resolves.toBeUndefined(); // ohne laufende Suche harmlos
  });

  test("eine neue Suche ersetzt die laufende", async () => {
    const { api } = setup();
    const first = api.findInFile(`${HOME}/große-datei.bin`, needle("unwahrscheinlich-9999"), 0, "forward", false);
    const firstOutcome = first.catch((error: unknown) => error);
    const second = await api.findInFile(NOTES, needle("Zeile 00000001"), 0, "forward", false);
    expect(second).toEqual({ kind: "found", offset: 64 });
    expect(await firstOutcome).toMatchObject({ kind: "cancelled" });
  });

  test("findInFile lehnt leere und zu lange Suchbytes ab", async () => {
    const { api } = setup();
    await expect(api.findInFile(NOTES, new Uint8Array(0), 0, "forward", false)).rejects.toMatchObject({ kind: "invalidInput" });
    await expect(api.findInFile(NOTES, new Uint8Array(4097), 0, "forward", false)).rejects.toMatchObject({ kind: "invalidInput" });
  });
});

describe("renameMany (Mehrfach-Umbenennen)", () => {
  const rename = async (api: DualfileApi, items: [string, string][]): Promise<JobStatus> =>
    jobOf(api, await api.renameMany(DOCS, items.map(([oldName, newName]) => ({ oldName, newName }))));

  test("benennt einfach um, meldet den Ordner und beendet den Auftrag mit „done“", async () => {
    const { api, events } = setup();
    const job = await rename(api, [["notizen.txt", "ideen.txt"]]);
    expect(job).toMatchObject({ state: "done", filesDone: 1, failed: [] });
    expect(job.name).toBe("Umbenennen: notizen.txt");
    expect(await sizeOf(api, DOCS, "ideen.txt")).toBe(1_200);
    expect(await names(api, DOCS)).not.toContain("notizen.txt");
    expect(changedPaths(events)).toContain(DOCS);
  });

  test("tauscht zwei Namen", async () => {
    const { api } = setup();
    const job = await rename(api, [
      ["notizen.txt", "Änderungen.txt"],
      ["Änderungen.txt", "notizen.txt"],
    ]);
    expect(job.state).toBe("done");
    expect(await sizeOf(api, DOCS, "notizen.txt")).toBe(4_000);
    expect(await sizeOf(api, DOCS, "Änderungen.txt")).toBe(1_200);
  });

  test("führt Ketten und Zyklen aus", async () => {
    const { api } = setup();
    // Kette: notizen -> Änderungen -> neu.txt
    await rename(api, [
      ["notizen.txt", "Änderungen.txt"],
      ["Änderungen.txt", "neu.txt"],
    ]);
    expect(await sizeOf(api, DOCS, "Änderungen.txt")).toBe(1_200);
    expect(await sizeOf(api, DOCS, "neu.txt")).toBe(4_000);
    // Zyklus über drei Namen
    const job = await rename(api, [
      ["Änderungen.txt", "neu.txt"],
      ["neu.txt", "Übersicht.ods"],
      ["Übersicht.ods", "Änderungen.txt"],
    ]);
    expect(job.state).toBe("done");
    expect(await sizeOf(api, DOCS, "neu.txt")).toBe(1_200);
    expect(await sizeOf(api, DOCS, "Übersicht.ods")).toBe(4_000);
    expect(await sizeOf(api, DOCS, "Änderungen.txt")).toBe(38_000);
  });

  test("überschreibt nie: Kollision mit einem nicht umbenannten Eintrag lehnt den ganzen Lauf ab", async () => {
    const { api } = setup();
    const before = await names(api, DOCS);
    const job = await rename(api, [
      ["notizen.txt", "Änderungen.txt"],
      ["Übersicht.ods", "frei.ods"],
    ]);
    expect(job.state).toBe("failed");
    expect(job.failed.map((f) => f.path)).toEqual([`${DOCS}/notizen.txt`]);
    expect(await names(api, DOCS)).toEqual(before);
    expect(await sizeOf(api, DOCS, "Änderungen.txt")).toBe(4_000);
  });

  test("doppelte Zielnamen und doppelte Quellen lehnen den Lauf ab", async () => {
    const { api } = setup();
    const before = await names(api, DOCS);
    const duplicateTarget = await rename(api, [
      ["notizen.txt", "x.txt"],
      ["Übersicht.ods", "x.txt"],
    ]);
    expect(duplicateTarget.state).toBe("failed");
    expect(duplicateTarget.failed).toHaveLength(2);
    const duplicateSource = await rename(api, [
      ["notizen.txt", "a.txt"],
      ["notizen.txt", "b.txt"],
    ]);
    expect(duplicateSource.state).toBe("failed");
    expect(await names(api, DOCS)).toEqual(before);
  });

  test("ungültige Namen werden mit „invalidName“ abgelehnt, ohne Auftrag", async () => {
    const { api } = setup();
    for (const bad of ["", ".", "..", "a/b"]) {
      await expect(api.renameMany(DOCS, [{ oldName: "notizen.txt", newName: bad }])).rejects.toMatchObject({
        kind: "invalidName",
      });
    }
    expect(await api.jobs()).toEqual([]);
    expect(await names(api, DOCS)).toContain("notizen.txt");
  });

  test("eine fehlende Quelle scheitert allein, der Rest läuft", async () => {
    const { api } = setup();
    const job = await rename(api, [
      ["fehlt.txt", "x.txt"],
      ["notizen.txt", "ideen.txt"],
    ]);
    expect(job.state).toBe("done");
    expect(job.failed.map((f) => f.path)).toEqual([`${DOCS}/fehlt.txt`]);
    expect(await sizeOf(api, DOCS, "ideen.txt")).toBe(1_200);
  });

  test("unveränderte Namen zählen nicht, blockieren aber ihren Namen", async () => {
    const { api } = setup();
    const same = await rename(api, [["notizen.txt", "notizen.txt"]]);
    expect(same).toMatchObject({ state: "done", filesDone: 0 });
    const blocked = await rename(api, [
      ["Übersicht.ods", "Änderungen.txt"],
      ["Änderungen.txt", "Änderungen.txt"],
    ]);
    expect(blocked.state).toBe("failed");
    expect(await sizeOf(api, DOCS, "Übersicht.ods")).toBe(38_000);
  });

  test("leere Liste und unbekannter Ordner werden abgelehnt", async () => {
    const { api } = setup();
    await expect(api.renameMany(DOCS, [])).rejects.toMatchObject({ kind: "invalidInput" });
    await expect(api.renameMany(`${HOME}/fehlt`, [{ oldName: "a", newName: "b" }])).rejects.toMatchObject({
      kind: "notFound",
    });
    await expect(api.renameMany("relativ", [{ oldName: "a", newName: "b" }])).rejects.toMatchObject({
      kind: "invalidPath",
    });
  });
});

describe("regexReplace (Mock, nur Entwurf)", () => {
  const ok = (name: string) => ({ kind: "ok", name });

  test("ersetzt mit Gruppen, Namen und „$$“", async () => {
    const { api } = setup();
    const out = await api.regexReplace(["IMG_2024_05.jpg", "x"], "IMG_(\\d+)_(?P<m>\\d+)", "$2-${1}", false, true);
    expect(out).toEqual([ok("05-2024.jpg"), ok("x")]);
    expect(await api.regexReplace(["a"], "a", "$$", false, true)).toEqual([ok("$")]);
    expect(await api.regexReplace(["ab"], "(?<x>a)", "[$x]", false, true)).toEqual([ok("[a]b")]);
  });

  test("beachtet Groß-/Kleinschreibung und erster/alle Treffer", async () => {
    const { api } = setup();
    expect(await api.regexReplace(["Foto.JPG"], "jpg", "png", false, true)).toEqual([ok("Foto.JPG")]);
    expect(await api.regexReplace(["Foto.JPG"], "jpg", "png", true, true)).toEqual([ok("Foto.png")]);
    expect(await api.regexReplace(["a-a-a"], "a", "b", false, false)).toEqual([ok("b-a-a")]);
    expect(await api.regexReplace(["a-a-a"], "a", "b", false, true)).toEqual([ok("b-b-b")]);
  });

  test("lehnt ungültige Muster, Rückverweise und Lookarounds mit „invalidPattern“ ab", async () => {
    const { api } = setup();
    for (const bad of ["(", "[a-", "", "(a)\\1", "(?=a)", "(?<!a)b"]) {
      await expect(api.regexReplace(["a"], bad, "x", false, true)).rejects.toMatchObject({ kind: "invalidPattern" });
    }
    await expect(api.regexReplace(["a"], "a".repeat(1001), "x", false, true)).rejects.toMatchObject({
      kind: "invalidPattern",
    });
    expect(await api.regexReplace(["a"], "a".repeat(1000), "x", false, true)).toHaveLength(1);
  });

  test("hält Mengen- und Längengrenzen ein", async () => {
    const { api } = setup();
    await expect(api.regexReplace(Array(10_001).fill("a"), "a", "b", false, true)).rejects.toMatchObject({
      kind: "invalidInput",
    });
    expect(await api.regexReplace(Array(10_000).fill("a"), "a", "b", false, true)).toHaveLength(10_000);
    const out = await api.regexReplace(["a".repeat(255), "a".repeat(256), "ä".repeat(128)], "a", "b", false, true);
    expect(out[0]?.kind).toBe("ok");
    expect(out[1]?.kind).toBe("error");
    expect(out[2]?.kind).toBe("error");
    const huge = await api.regexReplace(["a".repeat(200)], "a", "y".repeat(1000), false, true);
    expect(huge[0]?.kind).toBe("error");
  });
});

describe("Archive im Mock (Phase 4b)", () => {
  const ARCHIVE = `${DOWNLOADS}/rezept-paket.tar.gz`;
  const at = (inner: string, file = ARCHIVE): string => `archive:${file.replaceAll("%", "%25").replaceAll("!", "%21")}!${inner}`;

  test("das Demo-Archiv lässt sich wie ein Ordner öffnen und durchlaufen", async () => {
    const { api } = setup();
    const root = await api.list(at("/"));
    expect(root.path).toBe(at("/"));
    expect(root.entries.map((e) => e.name).sort()).toEqual(["README.md", "aktuell", "extras.zip", "rezepte"]);
    const deeper = await api.list(at("/rezepte/desserts/"));
    expect(deeper.path).toBe(at("/rezepte/desserts"));
    expect(deeper.entries.map((e) => e.name)).toEqual(["tiramisu.txt"]);
    expect(root.entries.find((e) => e.name === "aktuell")).toMatchObject({ kind: "symlink", linkTarget: "rezepte/pizza.txt" });
    // verschachteltes Archiv im Archiv
    const nested = at("/", at("/extras.zip"));
    expect(nested).toBe(`archive:archive:${ARCHIVE}%21/extras.zip!/`);
    expect((await api.list(nested)).entries.map((e) => e.name)).toEqual(["tipps.txt"]);
  });

  test("Lister-Funktionen laufen auch in Archiven", async () => {
    const { api } = setup();
    const file = at("/rezepte/pizza.txt");
    expect(await api.fileInfo(file)).toMatchObject({ size: 640, kind: "text" });
    expect((await api.readBlock(file, 0, 10)).length).toBe(10);
    expect(await api.findInFile(file, new TextEncoder().encode("Beispieltext"), 0, "forward", false)).toMatchObject({ kind: "found" });
    await expect(api.fileInfo(at("/aktuell"))).rejects.toMatchObject({ kind: "unsupported" });
    await expect(api.fileInfo(at("/rezepte"))).rejects.toBeInstanceOf(CoreError);
  });

  test("Namen mit ! und % im Archivpfad werden kodiert gelesen", async () => {
    const { api, scheduler } = setup();
    const id = await api.packArchive([`${DOCS}/notizen.txt`], DOWNLOADS, "we!rd%name.zip", "zip");
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    const path = at("/", `${DOWNLOADS}/we!rd%name.zip`);
    expect(path).toContain("we%21rd%25name.zip");
    expect((await api.list(path)).entries.map((e) => e.name)).toEqual(["notizen.txt"]);
  });

  test("ungültige oder gefälschte Archivpfade werden abgelehnt", async () => {
    const { api } = setup();
    for (const bad of [
      "archive:/x.zip",
      `archive:${ARCHIVE}!/../../etc`,
      `archive:${ARCHIVE}!relativ`,
      "archive:relativ.zip!/x",
      `archive:/a%zz.zip!/x`,
      "archive:archive:archive:archive:/a.zip%2521!/b%21!/c%21!/d!/e",
    ]) {
      await expect(api.list(bad), bad).rejects.toMatchObject({ kind: "invalidPath" });
    }
    await expect(api.list(at("/nicht-da"))).rejects.toMatchObject({ kind: "notFound" });
    await expect(api.list(at("/", `${DOWNLOADS}/gibt-es-nicht.zip`))).rejects.toMatchObject({ kind: "notFound" });
    // eine Datei, die kein Archiv ist
    await expect(api.list(at("/", `${DOCS}/notizen.txt`))).rejects.toMatchObject({ kind: "archive" });
    await expect(api.list(at("/", DOCS))).rejects.toMatchObject({ kind: "invalidPath" });
  });

  test("Schreiben in Archivpfaden: immer „readOnly“, nichts verändert sich", async () => {
    const { api, scheduler } = setup();
    const file = at("/rezepte/pizza.txt");
    const folder = at("/rezepte");
    const before = await names(api, folder);
    const calls: Promise<unknown>[] = [
      api.copy([`${DOCS}/notizen.txt`], folder),
      api.move([`${DOCS}/notizen.txt`], folder),
      api.move([file], DOCS),
      api.trash([file]),
      api.restoreFromTrash([file]),
      api.planDelete([file]),
      api.rename(file, "neu.txt"),
      api.renameMany(folder, [{ oldName: "pizza.txt", newName: "x.txt" }]),
      api.makeDir(folder, "neu"),
      api.packArchive([`${DOCS}/notizen.txt`], folder, "x.zip", "zip"),
    ];
    for (const call of calls) {
      await expect(call).rejects.toMatchObject({ kind: "readOnly", message: "Archive sind schreibgeschützt" });
    }
    await scheduler.runAll();
    expect(await names(api, folder)).toEqual(before);
    expect(await api.jobs()).toEqual([]);
    await expect(api.openExternal(file)).rejects.toMatchObject({ kind: "unsupported" });
  });

  test("Kopieren aus dem Archiv entpackt als Auftrag („Entpacken“), auch ein ganzer Ordner", async () => {
    const { api, scheduler, events } = setup();
    const id = await api.copy([at("/rezepte"), at("/README.md")], DOCS);
    expect((await jobOf(api, id)).name).toBe("Entpacken: rezepte (+1)");
    await scheduler.runAll();
    const job = await jobOf(api, id);
    expect(job.state).toBe("done");
    expect(job.filesDone).toBe(4);
    expect(await names(api, `${DOCS}/rezepte/desserts`)).toEqual(["tiramisu.txt"]);
    expect(await sizeOf(api, DOCS, "README.md")).toBe(300);
    expect(changedPaths(events)).toContain(DOCS);
    // Quellen aus verschiedenen Orten sind nicht erlaubt
    await expect(api.copy([at("/README.md"), `${DOCS}/notizen.txt`], DOWNLOADS)).rejects.toMatchObject({ kind: "invalidInput" });
    await expect(api.copy([at("/")], DOCS)).rejects.toMatchObject({ kind: "invalidPath" });
  });

  test("ein entpacktes Archiv bleibt ein Archiv (Kopie mit Inhalt)", async () => {
    const { api, scheduler } = setup();
    const id = await api.copy([ARCHIVE], DOCS);
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    expect((await api.list(at("/rezepte", `${DOCS}/rezept-paket.tar.gz`))).entries).toHaveLength(3);
  });

  test("packArchive: Rundlauf, Fortschritt, Ordner-Ereignis und danach lesbar", async () => {
    const { api, scheduler, events } = setup();
    for (const format of ["zip", "tarGz", "tarBz2", "tarXz", "sevenZ"] as const) {
      const name = `paket-${format}`;
      const id = await api.packArchive([`${HOME}/Projekte/dualfile`, `${DOCS}/notizen.txt`], DOWNLOADS, name, format);
      expect((await jobOf(api, id)).name).toBe("Packen: dualfile (+1)");
      await scheduler.runAll();
      const job = await jobOf(api, id);
      expect(job.state, format).toBe("done");
      expect(job.filesDone).toBe(3);
      expect(job.bytesDone).toBe(12_000 + 2_500 + 1_200);
      const listing = await api.list(at("/", `${DOWNLOADS}/${name}`));
      expect(listing.entries.map((e) => e.name).sort()).toEqual(["dualfile", "notizen.txt"]);
      expect((await api.list(at("/dualfile/src", `${DOWNLOADS}/${name}`))).entries[0]?.name).toBe("main.ts");
    }
    expect(changedPaths(events)).toContain(DOWNLOADS);
    // Quelle bleibt unverändert
    expect(await sizeOf(api, DOCS, "notizen.txt")).toBe(1_200);
  });

  test("packArchive ersetzt nie ein vorhandenes Archiv und prüft die Eingaben", async () => {
    const { api } = setup();
    await expect(api.packArchive([`${DOCS}/notizen.txt`], DOWNLOADS, "rezept-paket.tar.gz", "tarGz")).rejects.toMatchObject({
      kind: "exists",
    });
    expect(await sizeOf(api, DOWNLOADS, "rezept-paket.tar.gz")).toBe(730_000);
    await expect(api.packArchive([`${DOCS}/notizen.txt`], DOWNLOADS, "a/b.zip", "zip")).rejects.toMatchObject({ kind: "invalidName" });
    await expect(api.packArchive([`${DOCS}/notizen.txt`], `${HOME}/nicht-da`, "x.zip", "zip")).rejects.toMatchObject({ kind: "notFound" });
    await expect(api.packArchive([`${DOCS}/notizen.txt`], `${DOCS}/notizen.txt`, "x.zip", "zip")).rejects.toMatchObject({
      kind: "notDirectory",
    });
    await expect(api.packArchive([], DOWNLOADS, "x.zip", "zip")).rejects.toMatchObject({ kind: "invalidPath" });
    await expect(api.packArchive([`${DOCS}/fehlt.txt`], DOWNLOADS, "x.zip", "zip")).rejects.toMatchObject({ kind: "notFound" });
    await expect(api.packArchive([`${DOCS}/notizen.txt`], DOWNLOADS, "x.rar", "rar" as never)).rejects.toMatchObject({
      kind: "invalidInput",
    });
  });

  test("packArchive: Ziel in der Quelle scheitert im Auftrag, Abbruch hinterlässt kein Archiv", async () => {
    const { api, scheduler } = setup();
    const inside = await api.packArchive([`${HOME}/Projekte`], `${HOME}/Projekte/dualfile`, "in.zip", "zip");
    await scheduler.runAll();
    const failed = await jobOf(api, inside);
    expect(failed.state).toBe("failed");
    expect(failed.failed[0]?.message).toBe("Der Zielordner liegt innerhalb der Quelle");
    expect(await names(api, `${HOME}/Projekte/dualfile`)).not.toContain("in.zip");

    const cancelled = await api.packArchive([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`], DOWNLOADS, "c.zip", "zip");
    await scheduler.step();
    await api.cancelJob(cancelled);
    await scheduler.runAll();
    expect((await jobOf(api, cancelled)).state).toBe("cancelled");
    expect(await names(api, DOWNLOADS)).not.toContain("c.zip");
  });

  test("packen aus einem Archiv heraus: Quellen im Archiv, Ziel lokal", async () => {
    const { api, scheduler } = setup();
    const id = await api.packArchive([at("/rezepte")], DOCS, "rezepte.tar.xz", "tarXz");
    await scheduler.runAll();
    expect((await jobOf(api, id)).state).toBe("done");
    expect((await api.list(at("/rezepte", `${DOCS}/rezepte.tar.xz`))).entries.map((e) => e.name).sort()).toEqual([
      "brot.txt",
      "desserts",
      "pizza.txt",
    ]);
  });
});

describe("undo history", () => {
  test("records all five operations and undoes them in the mock filesystem", async () => {
    const { api, scheduler } = setup();
    let notifications = 0;
    const stop = api.onUndoChanged(() => notifications++);
    const folder = await api.makeDir(HOME, "undo-target");
    await api.copy([`${DOCS}/notizen.txt`], folder);
    await scheduler.runAll();
    await api.move([`${folder}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.rename(`${DOWNLOADS}/notizen.txt`, "undo-name.txt");
    await api.trash([`${DOWNLOADS}/undo-name.txt`]);
    const entries = await api.undoList();
    expect(entries.map((e) => e.kind)).toEqual(["trash", "rename", "move", "copy", "mkdir"]);
    expect(notifications).toBe(5);
    for (const entry of entries) {
      const pending = api.undoRun(entry.id);
      await scheduler.runAll();
      expect(await pending).toMatchObject({ ok: true, errors: [], remaining: 0 });
    }
    expect(await names(api, HOME)).not.toContain("undo-target");
    expect(await names(api, DOCS)).toContain("notizen.txt");
    expect((await api.undoList()).every((e) => e.undone)).toBe(true);
    expect(notifications).toBe(10);
    stop();
    await api.makeDir(HOME, "after-stop");
    expect(notifications).toBe(10);
    expect((await api.undoRun(entries[0]!.id)).errors).toEqual(["alreadyUndone"]);
    expect((await api.undoRun(9999)).errors).toEqual(["notFound"]);
  });

  test("changed, conflict and cancellation are fixed error results", async () => {
    const { api, scheduler } = setup();
    await api.rename(`${DOCS}/notizen.txt`, "renamed.txt");
    const id = (await api.undoList())[0]!.id;
    await api.makeDir(DOCS, "notizen.txt");
    let pending = api.undoRun(id);
    await scheduler.runAll();
    expect((await pending).errors).toEqual(["conflict"]);
    await api.trash([`${DOCS}/renamed.txt`]);
    pending = api.undoRun(id);
    await scheduler.runAll();
    expect((await pending).errors).toEqual(["changed"]);
    const dir = await api.makeDir(HOME, "nonempty");
    const mkdirId = (await api.undoList())[0]!.id;
    await api.makeDir(dir, "child");
    pending = api.undoRun(mkdirId);
    await scheduler.runAll();
    expect((await pending).errors).toEqual(["changed"]);
    pending = api.undoRun(mkdirId);
    const job = (await api.jobs()).at(-1)!;
    await api.cancelJob(job.id);
    await scheduler.runAll();
    expect(await pending).toMatchObject({ cancelled: true, ok: false });
  });

  test("replacement excluded; keepBoth records actual destination; no permanent delete history", async () => {
    const { api, scheduler } = setup();
    await makeConflictTarget(api);
    const count = (await api.undoList()).length;
    const off = api.onEvent((e) => { if (e.type === "conflict") void api.answerConflict(e.conflict.jobId, { resolution: "overwrite", applyToAll: true }); });
    await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    expect(await api.undoList()).toHaveLength(count);
    off();
    api.onEvent((e) => { if (e.type === "conflict") void api.answerConflict(e.conflict.jobId, { resolution: "keepBoth", applyToAll: true }); });
    await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    const undo = api.undoRun((await api.undoList())[0]!.id);
    await scheduler.runAll();
    expect((await undo).ok).toBe(true);
    expect((await api.list(DOWNLOADS)).entries.filter((e) => e.name.startsWith("notizen"))).toHaveLength(1);
    const before = (await api.undoList()).length;
    const plan = await api.planDelete([`${DOWNLOADS}/notizen.txt`]);
    await api.deletePermanent(plan, plan.token);
    await scheduler.runAll();
    expect(await api.undoList()).toHaveLength(before);
  });

  test("simple multirename is one record, cycles are omitted; capacity is 200", async () => {
    const { api } = setup();
    await api.renameMany(DOWNLOADS, [{ oldName: "Datei1.txt", newName: "one.txt" }, { oldName: "Datei2.txt", newName: "two.txt" }]);
    expect((await api.undoList()).map((e) => e.kind)).toEqual(["rename"]);
    await api.renameMany(DOWNLOADS, [{ oldName: "one.txt", newName: "two.txt" }, { oldName: "two.txt", newName: "one.txt" }]);
    expect(await api.undoList()).toHaveLength(1);
    for (let i = 0; i < 201; i++) await api.makeDir(HOME, `bounded-${i}`);
    const entries = await api.undoList();
    expect(entries).toHaveLength(200);
    expect(entries[0]!.id).toBeGreaterThan(entries[199]!.id);
  });
});

test("undo ignores archive copies and detects a modified copied file", async () => {
  const { api, scheduler } = setup();
  const folder = await api.makeDir(HOME, "archive-output");
  const before = (await api.undoList()).length;
  await api.copy([`archive:${DOWNLOADS}/rezept-paket.tar.gz!/README.md`], folder);
  await scheduler.runAll();
  expect(await api.undoList()).toHaveLength(before);
  await api.copy([`${DOCS}/notizen.txt`], folder);
  await scheduler.runAll();
  const id = (await api.undoList())[0]!.id;
  await api.rename(`${DOWNLOADS}/Datei2.txt`, "notizen.txt");
  api.onEvent((e) => { if (e.type === "conflict") void api.answerConflict(e.conflict.jobId, { resolution: "overwrite", applyToAll: false }); });
  await api.copy([`${DOWNLOADS}/notizen.txt`], folder);
  await scheduler.runAll();
  const undo = api.undoRun(id);
  await scheduler.runAll();
  expect((await undo).errors).toEqual(["changed"]);
});


test("undo retries only remaining items after partial success", async () => {
  const { api, scheduler } = setup();
  await api.renameMany(DOWNLOADS, [{ oldName: "Datei1.txt", newName: "one.txt" }, { oldName: "Datei2.txt", newName: "two.txt" }]);
  const id = (await api.undoList())[0]!.id;
  await api.makeDir(DOWNLOADS, "Datei2.txt");
  const first = api.undoRun(id);
  await scheduler.runAll();
  expect(await first).toMatchObject({ ok: false, done: 1, remaining: 1, errors: ["conflict"] });
  await api.trash([`${DOWNLOADS}/Datei2.txt`]);
  const retry = api.undoRun(id);
  await scheduler.runAll();
  expect(await retry).toMatchObject({ ok: true, done: 1, remaining: 0, errors: [] });
  expect(await names(api, DOWNLOADS)).toEqual(expect.arrayContaining(["Datei1.txt", "Datei2.txt"]));
});

describe("Phase 10 jobs", () => {
  test("pause stops bytes, reserves the destination lane, resume finishes FIFO with ETA", async () => {
    const { api, scheduler, events } = setup();
    const first = await api.copy([`${HOME}/große-datei.bin`], DOWNLOADS);
    const second = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.step();
    await scheduler.step();
    await scheduler.step();
    expect((await jobOf(api, first)).lane).toBe((await jobOf(api, second)).lane);
    expect((await jobOf(api, first)).lane).not.toContain("/");
    await api.jobPause(first);
    const bytes = (await jobOf(api, first)).bytesDone;
    await scheduler.runAll();
    expect((await jobOf(api, first)).state).toBe("paused");
    expect((await jobOf(api, first)).bytesDone).toBe(bytes);
    expect((await jobOf(api, second)).state).toBe("queued");
    expect((await jobOf(api, second)).queuePosition).toBeGreaterThan(0);
    expect((await jobOf(api, second)).bytesDone).toBe(0);
    await api.jobResume(first);
    await scheduler.settle();
    await scheduler.runAll();
    expect((await jobOf(api, first)).state).toBe("done");
    expect((await jobOf(api, second)).state).toBe("done");
    expect(events.some(e => e.type === "job" && e.status.id === first && e.status.etaSeconds !== null && e.status.etaSeconds > 0)).toBe(true);
    expect((await jobOf(api, first)).etaSeconds).toBeNull();
  });

  test("cancel wakes paused jobs and releases their lane", async () => {
    const { api, scheduler } = setup();
    const first = await api.copy([`${HOME}/große-datei.bin`], DOWNLOADS);
    await api.jobPause(first);
    const second = await api.copy([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    await api.cancelJob(first);
    await scheduler.settle();
    await scheduler.runAll();
    expect((await jobOf(api, second)).state).toBe("done");
    await expect(api.jobPause(999)).rejects.toMatchObject({ kind: "invalidInput" });
  });

  test("Always corruption demo reports verifyFailed and retains move source and destination", async () => {
    const scheduler = new ManualScheduler();
    const api = createMockApi({ scheduler, verifyFailure: true });
    const loaded = await api.loadSettings();
    await api.saveSettings({ ...loaded.settings, verifyPolicy: "always" });
    const id = await api.move([`${DOCS}/notizen.txt`], DOWNLOADS);
    await scheduler.runAll();
    expect((await jobOf(api, id)).failed[0]).toMatchObject({ kind: "verifyFailed", message: "Die Prüfsumme stimmt nach dem Übertragen nicht überein" });
    expect(await names(api, DOCS)).toContain("notizen.txt");
    expect(await names(api, DOWNLOADS)).toContain("notizen.txt");
    await expect(api.saveSettings({ ...loaded.settings, verifyPolicy: "invalid" as Settings["verifyPolicy"] })).rejects.toMatchObject({ kind: "invalidSettings" });
  });
});

describe("copyName (#46)", () => {
  test("Endungen, .tar.gz und Zählung wie im Kern", async () => {
    const { copyName } = await import("./mock");
    const none = () => false;
    expect(copyName("foto.jpg", false, none)).toBe("foto_copy.jpg");
    expect(copyName("foto.jpg", false, (c) => c === "foto_copy.jpg")).toBe("foto_copy2.jpg");
    expect(copyName("a.tar.gz", false, none)).toBe("a_copy.tar.gz");
    expect(copyName(".bashrc", false, none)).toBe(".bashrc_copy");
    expect(copyName("v1.2", true, none)).toBe("v1.2_copy");
    expect(copyName("foto_copy.jpg", false, (c) => c === "foto_copy.jpg")).toBe("foto_copy2.jpg");
    expect(copyName("a_copy1.txt", false, none)).toBe("a_copy1_copy.txt");
  });
});
