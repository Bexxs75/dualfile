import { describe, expect, it } from "vitest";
import { createTranslator } from "../i18n";
import type { CoreEvent, DeletePlan, Entry, JobStatus } from "../ipc/types";
import {
  canConfirmDelete,
  failureReason,
  jobDisplayName,
  closeJob,
  confirmDelete,
  deleteDialogModel,
  emptyJobs,
  jobsReducer,
  planCopy,
  planMove,
  summarizeJob,
  withDestination,
  type PaneSnapshot,
} from "./operations";

const e = (name: string, extra: Partial<Entry> = {}): Entry => ({
  name,
  kind: "file",
  size: 100,
  modified: 0,
  mode: null,
  hidden: false,
  linkTarget: null,
  brokenLink: false,
  error: null,
  ...extra,
});
const entries = [e("..", { kind: "dir", size: 0 }), e("a.txt", { size: 10 }), e("b.txt", { size: 20 }), e("sub", { kind: "dir", size: 4096 })];
const pane = (extra: Partial<PaneSnapshot> = {}): PaneSnapshot => ({
  path: "/home/a",
  entries,
  selected: new Set(),
  focus: 1,
  ...extra,
});
const other: PaneSnapshot = pane({ path: "/mnt/ziel/" });

describe("planCopy / planMove", () => {
  it("nimmt als Ziel immer den Pfad des anderen Fensters", () => {
    expect(planCopy(pane(), other)?.destination).toBe("/mnt/ziel");
    expect(planMove(pane(), other)?.destination).toBe("/mnt/ziel");
    expect(planCopy(pane(), other)?.kind).toBe("copy");
    expect(planMove(pane(), other)?.kind).toBe("move");
  });
  it("nimmt ohne Markierung den Fokuseintrag", () => {
    const plan = planCopy(pane({ focus: 2 }), other)!;
    expect(plan.sources).toEqual(["/home/a/b.txt"]);
    expect(plan.count).toBe(1);
    expect(plan.bytes).toBe(20);
  });
  it("nimmt mit Markierung die markierten Einträge (nicht den Fokus) in Listenreihenfolge", () => {
    const plan = planCopy(pane({ selected: new Set(["sub", "a.txt"]), focus: 2 }), other)!;
    expect(plan.sourceNames).toEqual(["a.txt", "sub"]);
    expect(plan.count).toBe(2);
    expect(plan.fileCount).toBe(1);
    expect(plan.folderCount).toBe(1);
  });
  it("zählt nur Dateigrößen und kennzeichnet unbekannte Ordnergrößen", () => {
    const plan = planCopy(pane({ selected: new Set(["a.txt", "b.txt", "sub"]) }), other)!;
    expect(plan.bytes).toBe(30);
    expect(plan.bytesIncomplete).toBe(true);
    expect(planCopy(pane({ selected: new Set(["a.txt"]) }), other)!.bytesIncomplete).toBe(false);
  });
  it("tut bei Fokus auf .. ohne Markierung nichts", () => {
    expect(planCopy(pane({ focus: 0 }), other)).toBeNull();
    expect(planMove(pane({ focus: 0 }), other)).toBeNull();
  });
  it("nimmt .. nie mit, auch wenn der Name irrtümlich in der Markierung steht", () => {
    const plan = planCopy(pane({ selected: new Set(["..", "a.txt"]) }), other)!;
    expect(plan.sourceNames).toEqual(["a.txt"]);
    expect(planCopy(pane({ selected: new Set([".."]), focus: 0 }), other)).toBeNull();
  });
  it("liefert null bei leerem Ordner", () => {
    expect(planCopy(pane({ entries: [], focus: 0 }), other)).toBeNull();
  });
  it("baut Pfade auch an der Wurzel richtig", () => {
    expect(planCopy(pane({ path: "/" }), other)!.sources).toEqual(["/a.txt"]);
  });
  it("meldet gleiches Quell- und Zielverzeichnis", () => {
    expect(planCopy(pane(), pane({ path: "/home/a/" }))!.sameFolder).toBe(true);
    expect(planCopy(pane(), other)!.sameFolder).toBe(false);
  });
  it("erlaubt, das Ziel im Dialog zu ändern, und lehnt leeres Ziel ab", () => {
    const plan = planCopy(pane(), other)!;
    const changed = withDestination(plan, "/neu/ziel/", "/home/a")!;
    expect(changed.destination).toBe("/neu/ziel");
    expect(changed.sources).toEqual(plan.sources);
    expect(withDestination(plan, "/home/a", "/home/a")!.sameFolder).toBe(true);
    expect(withDestination(plan, "  ", "/home/a")).toBeNull();
  });
});

describe("Löschdialog", () => {
  const plan: DeletePlan = { token: "tok-1", files: 3, dirs: 1, links: 1, bytes: 999, firstNames: ["a", "b", "c"], incomplete: false };
  it("hat „Abbrechen“ als Standardknopf", () => {
    const model = deleteDialogModel(plan);
    expect(model.defaultButton).toBe("cancel");
    expect(model.buttons).toEqual(["cancel", "confirm"]);
  });
  it("zeigt Zähler, erste Namen und Rest", () => {
    const model = deleteDialogModel(plan);
    expect(model).toMatchObject({ files: 3, dirs: 1, links: 1, bytes: 999, total: 5, moreCount: 2, firstNames: ["a", "b", "c"] });
  });
  it("trägt einen Hinweis, dass es nicht rückgängig zu machen ist", () => {
    const model = deleteDialogModel(plan);
    expect(model.warning).toMatch(/nicht rückgängig/);
    expect(model.warningKey).toBe("dialog.delete.warning");
  });
  it("bestätigt nur mit dem Token des Plans", () => {
    const model = deleteDialogModel(plan);
    expect(canConfirmDelete(model, "tok-1")).toBe(true);
    expect(canConfirmDelete(model, "tok-2")).toBe(false);
    expect(canConfirmDelete(model, "")).toBe(false);
    expect(confirmDelete(model, "tok-1")).toBe("tok-1");
    expect(confirmDelete(model, "falsch")).toBeNull();
  });
  it("ein Plan ohne Token ist nie bestätigbar", () => {
    const model = deleteDialogModel({ ...plan, token: "" });
    expect(canConfirmDelete(model, "")).toBe(false);
  });
  it("gibt incomplete weiter", () => {
    expect(deleteDialogModel({ ...plan, incomplete: true }).incomplete).toBe(true);
  });
});

describe("jobsReducer", () => {
  const job = (extra: Partial<JobStatus> = {}): JobStatus => ({
    id: 1,
    name: "Kopieren",
    state: "running",
    filesDone: 0,
    bytesDone: 0,
    bytesTotal: 1000,
    bytesPerSecond: 0, etaSeconds: null, queuePosition: null, lane: null,
    failed: [],
    skipped: [],
    warnings: [],
    ...extra,
  });
  const ev = (status: JobStatus): CoreEvent => ({ type: "job", status });

  it("legt Aufträge an und aktualisiert sie", () => {
    let s = jobsReducer(emptyJobs, ev(job()));
    s = jobsReducer(s, ev(job({ id: 2, name: "Verschieben" })));
    s = jobsReducer(s, ev(job({ bytesDone: 500 })));
    expect(s.jobs.map((j) => j.id)).toEqual([1, 2]);
    expect(s.jobs[0]!.bytesDone).toBe(500);
  });
  it("hält den Fortschritt monoton", () => {
    let s = jobsReducer(emptyJobs, ev(job({ bytesDone: 600, filesDone: 6 })));
    s = jobsReducer(s, ev(job({ bytesDone: 400, filesDone: 4 })));
    expect(s.jobs[0]).toMatchObject({ bytesDone: 600, filesDone: 6 });
  });
  it("lässt einen beendeten Auftrag beendet, auch bei verspätetem running-Ereignis", () => {
    let s = jobsReducer(emptyJobs, ev(job({ state: "done", bytesDone: 1000 })));
    s = jobsReducer(s, ev(job({ state: "running", bytesDone: 800 })));
    expect(s.jobs[0]).toMatchObject({ state: "done", bytesDone: 1000 });
  });
  it("lässt fertige Aufträge sichtbar, bis sie geschlossen werden", () => {
    let s = jobsReducer(emptyJobs, ev(job({ state: "done" })));
    expect(s.jobs).toHaveLength(1);
    s = closeJob(s, 1);
    expect(s.jobs).toHaveLength(0);
  });
  it("schließt keinen laufenden Auftrag", () => {
    const s = jobsReducer(emptyJobs, ev(job()));
    expect(closeJob(s, 1)).toBe(s);
  });
  it("öffnet einen geschlossenen Auftrag nicht durch ein spätes Ereignis wieder", () => {
    let s = jobsReducer(emptyJobs, ev(job({ state: "cancelled" })));
    s = closeJob(s, 1);
    s = jobsReducer(s, ev(job({ state: "cancelled", bytesDone: 1 })));
    expect(s.jobs).toHaveLength(0);
  });
  it("fasst Fehler zusammen: 3 von 120 nicht kopiert, mit Gründen", () => {
    const failed = [
      { path: "/a", message: "Zugriff verweigert" },
      { path: "/b", message: "Kein Platz" },
      { path: "/c", message: "Datei zu lang" },
    ];
    const s = jobsReducer(emptyJobs, ev(job({ state: "failed", filesDone: 117, failed })));
    const sum = summarizeJob(s.jobs[0]!);
    expect(sum.failedCount).toBe(3);
    expect(sum.totalCount).toBe(120);
    expect(sum.failures.map((f) => f.message)).toEqual(["Zugriff verweigert", "Kein Platz", "Datei zu lang"]);
  });
  it("lässt die Fehlerliste nicht schrumpfen", () => {
    const f = [{ path: "/a", message: "x" }];
    let s = jobsReducer(emptyJobs, ev(job({ failed: f })));
    s = jobsReducer(s, ev(job({ failed: [] })));
    expect(s.jobs[0]!.failed).toHaveLength(1);
  });
  it("berechnet den Fortschritt begrenzt auf 0…1 und 1 bei done", () => {
    expect(summarizeJob(job({ bytesDone: 250 })).progress).toBe(0.25);
    expect(summarizeJob(job({ bytesTotal: 0 })).progress).toBe(0);
    expect(summarizeJob(job({ bytesDone: 5000 })).progress).toBe(1);
    expect(summarizeJob(job({ state: "done", bytesTotal: 0 })).progress).toBe(1);
  });
  it("ignoriert Ereignisse, die keine Aufträge sind", () => {
    const s = jobsReducer(emptyJobs, { type: "dirChanged", path: "/x" });
    expect(s).toBe(emptyJobs);
  });
});

describe("Auftragsnamen und Fehlergründe aus i18n", () => {
  const de = createTranslator("de");
  const en = createTranslator("en");
  const named = (title: { kind: string; first: string; more: number } | undefined) => ({ name: "Kopieren: x (+2)", title });
  it("bildet den Namen je Sprache aus kind, first und more", () => {
    expect(jobDisplayName(named({ kind: "copy", first: "x", more: 2 }), en)).toBe("Copy: x (+2)");
    expect(jobDisplayName(named({ kind: "copy", first: "x", more: 2 }), de)).toBe("Kopieren: x (+2)");
    expect(jobDisplayName(named({ kind: "move", first: "x", more: 0 }), en)).toBe("Move: x");
    expect(jobDisplayName(named({ kind: "similar", first: "", more: 0 }), en)).toBe("Similar images");
    expect(jobDisplayName(named({ kind: "similar", first: "", more: 0 }), de)).toBe("Ähnliche Bilder");
    expect(jobDisplayName(named({ kind: "space", first: "", more: 0 }), en)).toBe("Disk usage");
    expect(jobDisplayName(named({ kind: "dupes", first: "", more: 0 }), en)).toBe("Find duplicates");
    expect(jobDisplayName(named({ kind: "dupesTrash", first: "", more: 0 }), en)).toBe("Trash: duplicates");
    expect(jobDisplayName(named({ kind: "extract", first: "a.zip", more: 0 }), en)).toBe("Extract: a.zip");
  });
  it("fällt ohne title oder bei unbekannter Art auf den Namen des Kerns zurück", () => {
    expect(jobDisplayName(named(undefined), en)).toBe("Kopieren: x (+2)");
    expect(jobDisplayName(named({ kind: "zukunft", first: "x", more: 0 }), en)).toBe("Kopieren: x (+2)");
  });
  it("übersetzt feste Fehlerarten und fällt sonst auf den Text des Kerns zurück", () => {
    expect(failureReason({ path: "/p", message: "Zugriff verweigert: /p", kind: "permission" }, en)).toBe("Access denied.");
    expect(failureReason({ path: "/p", message: "Nicht gefunden: /p", kind: "notFound" }, en)).toBe("Not found: /p");
    expect(failureReason({ path: "/p", message: "Zugriff verweigert: /p", kind: "permission" }, de)).toBe("Zugriff verweigert.");
    expect(failureReason({ path: "/p", message: "Roher Text", kind: "io" }, en)).toBe("Roher Text");
    expect(failureReason({ path: "/p", message: "Roher Text" }, en)).toBe("Roher Text");
  });
});
