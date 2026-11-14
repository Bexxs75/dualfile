import { describe, expect, test } from "vitest";
import { CoreError, type DualfileApi } from "./api";
import { createMockApi, MOCK_SEARCH_FILE_BYTES, type MockScheduler } from "./mock";
import { compilePatterns, excerptOf, globSource, scanText } from "./mock-search";
import {
  MAX_SEARCH_EXCERPT_CHARS,
  MAX_SEARCH_PAGE,
  MAX_SEARCHES,
  type CoreEvent,
  type JobStatus,
  type SearchOptions,
} from "./types";

const HOME = "/home/demo";

/** Von Hand gesteuerter Zeitgeber wie in `mock-compare.test.ts`. */
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
  const api = createMockApi({ scheduler, now: () => Date.UTC(2026, 10, 12, 12, 0), samples: true });
  const events: CoreEvent[] = [];
  api.onEvent((event) => events.push(event));
  return { api, scheduler, events };
}

const jobOf = async (api: DualfileApi, id: number): Promise<JobStatus> => {
  const job = (await api.jobs()).find((candidate) => candidate.id === id);
  if (!job) throw new Error(`Auftrag ${id} fehlt`);
  return job;
};

/** Startet eine Suche und wartet, bis der Auftrag fertig ist; liefert die Ergebnis-Id. */
async function search(s: ReturnType<typeof setup>, root: string, options: SearchOptions): Promise<number> {
  const id = await s.api.searchStart(root, options);
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

describe("Bausteine", () => {
  test("Glob: ganze Namen, Alternativen, Klassen, wörtliche Sonderzeichen", () => {
    const names = (options: SearchOptions, list: string[]): string[] => {
      const re = compilePatterns(options).name!;
      return list.filter((name) => re.test(name));
    };
    const files = ["a.txt", "b.md", "main.rs", "Cargo.toml", "A.TXT", "a+b(1).txt"];
    expect(names({ name: "*.rs;*.toml" }, files)).toEqual(["main.rs", "Cargo.toml"]);
    expect(names({ name: "?.txt" }, files)).toEqual(["a.txt", "A.TXT"]);
    expect(names({ name: "[a-b].*" }, files)).toEqual(["a.txt", "b.md", "A.TXT"]);
    expect(names({ name: "[!a-b].*" }, files)).toEqual([]);
    expect(names({ name: "main" }, files)).toEqual([]);
    expect(names({ name: "*.txt", nameCaseSensitive: true }, files)).toEqual(["a.txt", "a+b(1).txt"]);
    expect(names({ name: "a+b(1).txt" }, files)).toEqual(["a+b(1).txt"]);
    expect(names({ name: "[x" }, ["[x", "x"])).toEqual(["[x"]);
    expect(globSource("a\\*b")).toBe("a\\*b");
    expect(names({ name: "^[a-z]+\\.rs$", nameMode: "regex" }, files)).toEqual(["main.rs"]);
  });

  test("Muster: leer, zu lang, ungültig, Rückverweise und Lookarounds werden abgelehnt", () => {
    const bad = (options: SearchOptions): void => {
      expect(() => compilePatterns(options)).toThrowError(CoreError);
      try {
        compilePatterns(options);
      } catch (error) {
        expect((error as CoreError).kind).toBe("invalidInput");
      }
    };
    bad({});
    bad({ name: "", text: "" });
    bad({ text: "a".repeat(1025) });
    bad({ name: "a\0b" });
    bad({ text: "(", textRegex: true });
    bad({ text: "(a)\\1", textRegex: true });
    bad({ text: "(?=a)", textRegex: true });
    bad({ name: "(", nameMode: "regex" });
    expect(compilePatterns({ text: "a.b(" }).text!.test("xa.b(y")).toBe(true);
    expect(compilePatterns({ text: "a.b(" }).text!.test("axb(")).toBe(false);
  });

  test("Zeilensuche: Nummer, CRLF, letzte Zeile, Binär, Groß/Klein", () => {
    const enc = (text: string): Uint8Array => new TextEncoder().encode(text);
    const re = compilePatterns({ text: "needle" }).text!;
    const scan = scanText(enc("a\r\nNEEDLE x\r\nend needle"), re, 10);
    expect(scan.hits.map((h) => [h.line, h.excerpt])).toEqual([
      [2, "NEEDLE x"],
      [3, "end needle"],
    ]);
    expect(scanText(Uint8Array.from([110, 0, 110]), re, 10).binary).toBe(true);
    expect(scanText(enc("needle\nneedle\n"), re, 1)).toMatchObject({ moreHits: true });
    expect(scanText(enc("needle\nneedle\n"), re, 2)).toMatchObject({ moreHits: false });
  });

  test("Auszug: gekürzt um den Treffer, Steuerzeichen ersetzt", () => {
    const line = `${"a".repeat(500)}NEEDLE${"b".repeat(500)}`;
    const found = excerptOf(line, 500, 506);
    const chars = Array.from(found.excerpt);
    expect(chars.length).toBeLessThanOrEqual(MAX_SEARCH_EXCERPT_CHARS + 2);
    expect(chars.slice(found.matchStart, found.matchStart + found.matchLen).join("")).toBe("NEEDLE");
    expect(found.excerpt.startsWith("…") && found.excerpt.endsWith("…")).toBe(true);
    expect(excerptOf("a\u001b[31mx\u0007", 0, 1).excerpt).toBe("a [31mx ");
  });
});

describe("searchStart / searchPage / searchClose", () => {
  test("Namenssuche läuft als Auftrag und liefert Seiten mit Übersicht", async () => {
    const s = setup();
    const id = await s.api.searchStart(HOME, { name: "Bericht*.txt" });
    // bevor der Auftrag fertig ist, gibt es noch kein Ergebnis
    await rejects(s.api.searchPage(id, 0, 10), "notFound");
    expect((await jobOf(s.api, id)).state).not.toBe("done");
    await s.scheduler.runAll();
    expect((await jobOf(s.api, id)).state).toBe("done");
    const page = await s.api.searchPage(id, 0, 100);
    expect(page.id).toBe(id);
    expect(page.hits.map((h) => h.path)).toEqual([
      `${HOME}/Dokumente/Bericht 2026-01.txt`,
      `${HOME}/Dokumente/Bericht 2026-02.txt`,
      `${HOME}/Dokumente/Bericht 2026-10.txt`,
    ]);
    expect(page.hits[0]).toMatchObject({ kind: "file", line: null, excerpt: null, size: 2_900 });
    expect(page.total).toBe(3);
    // der nicht lesbare Ordner der Vergleichs-Demo macht das Ergebnis unvollständig (nie still)
    expect(page.summary).toMatchObject({ root: HOME, total: 3, incomplete: true, cancelled: false, stopped: null });
    expect(page.summary.issues.every((issue) => issue.endsWith("Zugriff verweigert"))).toBe(true);
    expect(page.summary.dirsVisited).toBeGreaterThan(3);
    const second = await s.api.searchPage(id, 1, 1);
    expect(second.hits.map((h) => h.path)).toEqual([`${HOME}/Dokumente/Bericht 2026-02.txt`]);
    expect((await s.api.searchPage(id, 99, 10)).hits).toEqual([]);
  });

  test("Textsuche: Zeilennummer, Auszug, Binärdateien übersprungen, Filter und nicht rekursiv", async () => {
    const s = setup();
    const id = await search(s, `${HOME}/Ansicht`, { text: "köln" });
    const page = await s.api.searchPage(id, 0, 50);
    const hit = page.hits.find((h) => h.path.endsWith("/umlaute.txt"))!;
    expect(hit).toMatchObject({ line: 1, excerpt: "Grüße aus Köln", matchStart: 10, matchLen: 4 });
    expect(page.summary.skippedBinary).toBeGreaterThan(0);
    await s.api.searchClose(id);
    // Groß/Klein nur auf Wunsch
    const strict = await search(s, `${HOME}/Ansicht`, { text: "köln", textCaseSensitive: true });
    expect((await s.api.searchPage(strict, 0, 50)).total).toBe(0);
    await s.api.searchClose(strict);
    // Regulärer Ausdruck
    const regex = await search(s, `${HOME}/Ansicht`, { text: "Gr.{2}e aus", textRegex: true, name: "umlaute*" });
    expect((await s.api.searchPage(regex, 0, 50)).hits.map((h) => h.line)).toEqual([1]);
    await s.api.searchClose(regex);
    // Größenfilter
    const small = await search(s, `${HOME}/Dokumente`, { name: "*.txt", maxSize: 3_000 });
    const smallHits = (await s.api.searchPage(small, 0, 50)).hits;
    expect(smallHits.length).toBeGreaterThan(0);
    expect(smallHits.every((h) => h.size <= 3_000)).toBe(true);
    await s.api.searchClose(small);
    const flat = await search(s, HOME, { name: "Bericht*", recursive: false });
    expect((await s.api.searchPage(flat, 0, 50)).total).toBe(0);
  });

  test("ein Treffer im Archiv hat einen Archivpfad; ohne `inArchives` wird nicht hineingesehen", async () => {
    const s = setup();
    const without = await search(s, HOME, { text: "tiramisu" });
    expect((await s.api.searchPage(without, 0, 50)).total).toBe(0);
    await s.api.searchClose(without);
    const within = await search(s, HOME, { text: "tiramisu", inArchives: true });
    const page = await s.api.searchPage(within, 0, 50);
    expect(page.total).toBeGreaterThan(0);
    const hit = page.hits[0]!;
    expect(hit.path).toBe(`archive:${HOME}/Downloads/rezept-paket.tar.gz!/rezepte/desserts/tiramisu.txt`);
    expect(hit.line).toBe(1);
    expect(page.summary.archivesSearched).toBeGreaterThan(0);
    // der Pfad ist mit `list` im übergeordneten Ordner zu öffnen
    const listing = await s.api.list(`archive:${HOME}/Downloads/rezept-paket.tar.gz!/rezepte/desserts`);
    expect(listing.entries.map((e) => e.name)).toContain("tiramisu.txt");
    await s.api.searchClose(within);
    // Tiefe: das innere Archiv extras.zip (tipps.txt) braucht Ebene 2
    const one = await search(s, HOME, { name: "tipps.txt", inArchives: true, archiveDepth: 1 });
    expect((await s.api.searchPage(one, 0, 50)).total).toBe(0);
    await s.api.searchClose(one);
    const two = await search(s, HOME, { name: "tipps.txt", inArchives: true, archiveDepth: 2 });
    expect((await s.api.searchPage(two, 0, 50)).hits.map((h) => h.kind)).toEqual(["file"]);
  });

  test("Grenzen machen das Ergebnis unvollständig, mit Grund: Datei-, Byte- und Trefferbegrenzung", async () => {
    const s = setup();
    // 150-MiB-Textdatei: nur der Anfang wird gelesen
    const big = await search(s, `${HOME}/Ansicht`, { text: "Beispieltext", name: "riesig.txt" });
    const bigPage = await s.api.searchPage(big, 0, 10);
    expect(bigPage.summary.incomplete).toBe(true);
    expect(bigPage.summary.issues.some((i) => i.includes("riesig.txt") && i.includes(String(MOCK_SEARCH_FILE_BYTES)))).toBe(true);
    expect(bigPage.total).toBeGreaterThan(1000);
    await s.api.searchClose(big);
    // Treffergrenze (jede Zeile jeder Textdatei passt)
    const hits = await search(s, HOME, { text: "Beispieltext" });
    const hitsPage = await s.api.searchPage(hits, 0, 10);
    expect(hitsPage.summary).toMatchObject({ incomplete: true, stopped: "hits", total: 10_000 });
    expect(hitsPage.summary.issues.join(" ")).toContain("Treffer");
    await s.api.searchClose(hits);
    // nie still: ein Ergebnis ohne Grenze ist vollständig
    const small = await search(s, `${HOME}/Dokumente`, { name: "notizen*" });
    expect((await s.api.searchPage(small, 0, 10)).summary.incomplete).toBe(false);
  });

  test("nicht lesbare Ordner machen das Ergebnis unvollständig, nie leer", async () => {
    const s = setup();
    const id = await search(s, `${HOME}/Vergleich/links`, { name: "*.txt" });
    const page = await s.api.searchPage(id, 0, 100);
    expect(page.summary.incomplete).toBe(true);
    expect(page.summary.issues).toContain(`${HOME}/Vergleich/links/gesperrt: Zugriff verweigert`);
    expect(page.hits.some((h) => h.path.includes("/gesperrt/"))).toBe(false);
  });

  test("Abbruch: der Lauf behält seine bisherigen Treffer und ist als abgebrochen und unvollständig markiert", async () => {
    const s = setup();
    const id = await s.api.searchStart(HOME, { name: "*.txt" });
    await s.scheduler.settle();
    await s.scheduler.step();
    await s.scheduler.step();
    await s.api.cancelJob(id);
    await s.scheduler.runAll();
    expect((await jobOf(s.api, id)).state).toBe("cancelled");
    const page = await s.api.searchPage(id, 0, 500);
    await s.api.searchClose(id);
    expect(page.summary).toMatchObject({ cancelled: true, incomplete: true, stopped: "cancelled" });
    const full = await search(s, HOME, { name: "*.txt" });
    expect(page.total).toBeLessThan((await s.api.searchPage(full, 0, 500)).total);
  });

  test("Seiten sind auf MAX_SEARCH_PAGE begrenzt; Bereichsfehler und unbekannte Ids", async () => {
    const s = setup();
    const id = await search(s, `${HOME}/Ansicht`, { text: "Beispieltext", name: "riesig.txt" });
    expect((await s.api.searchPage(id, 0, 100_000)).hits).toHaveLength(MAX_SEARCH_PAGE);
    await rejects(s.api.searchPage(id, -1, 5), "invalidInput");
    await rejects(s.api.searchPage(id, 0, 1.5), "invalidInput");
    await rejects(s.api.searchPage(id + 100, 0, 5), "notFound");
  });

  test("Close gibt frei; höchstens MAX_SEARCHES Ergebnisse gleichzeitig", async () => {
    const s = setup();
    const ids: number[] = [];
    for (let i = 0; i < MAX_SEARCHES; i += 1) ids.push(await search(s, `${HOME}/Dokumente`, { name: "*.txt" }));
    const error = await rejects(s.api.searchStart(HOME, { name: "*.txt" }), "invalidInput");
    expect(error.message).toContain(String(MAX_SEARCHES));
    await s.api.searchClose(ids[0]!);
    await rejects(s.api.searchPage(ids[0]!, 0, 1), "notFound");
    await rejects(s.api.searchClose(ids[0]!), "notFound");
    const again = await search(s, `${HOME}/Dokumente`, { name: "*.txt" });
    expect((await s.api.searchPage(again, 0, 1)).total).toBe(5);
  });

  test("laufende Suchen zählen zu den Ergebnissen; abgelehnte Aufrufe belegen keinen Platz", async () => {
    const s = setup();
    const running: number[] = [];
    for (let i = 0; i < MAX_SEARCHES; i += 1) running.push(await s.api.searchStart(`${HOME}/Dokumente`, { name: "*" }));
    await rejects(s.api.searchStart(HOME, { name: "*" }), "invalidInput");
    await s.scheduler.runAll();
    for (const id of running) await s.api.searchClose(id);
    // ungültige Eingaben nehmen keinen Platz
    await rejects(s.api.searchStart(HOME, {}), "invalidInput");
    await rejects(s.api.searchStart(HOME, { text: "(", textRegex: true }), "invalidInput");
    for (let i = 0; i < MAX_SEARCHES; i += 1) await s.api.searchStart(HOME, { name: "*" });
  });

  test("ungültige Pfade, Optionen und Server ohne ausdrückliche Wahl werden vor dem Auftrag abgelehnt", async () => {
    const s = setup();
    const before = (await s.api.jobs()).length;
    await rejects(s.api.searchStart("relativ", { text: "x" }), "invalidPath");
    await rejects(s.api.searchStart("", { text: "x" }), "invalidPath");
    await rejects(s.api.searchStart(`${HOME}/fehlt`, { text: "x" }), "notFound");
    await rejects(s.api.searchStart(`${HOME}/Dokumente/notizen.txt`, { text: "x" }), "notDirectory");
    await rejects(s.api.searchStart(HOME, { text: "x", minSize: 10, maxSize: 5 }), "invalidInput");
    await rejects(s.api.searchStart(HOME, { text: "x", modifiedAfter: 10, modifiedBefore: 5 }), "invalidInput");
    await rejects(s.api.searchStart(HOME, { text: "x", archiveDepth: 0 }), "invalidInput");
    await rejects(s.api.searchStart(HOME, { text: "x", archiveDepth: 4 }), "invalidInput");
    await rejects(s.api.searchStart(HOME, { text: "x".repeat(2000) }), "invalidInput");
    await rejects(s.api.searchStart("ftp:abc!/x", { text: "x" }), "unsupported");
    await rejects(s.api.searchStart("ftp:abc!/x", { text: "x", includeRemote: true }), "disconnected");
    expect((await s.api.jobs()).length).toBe(before);
  });
});
