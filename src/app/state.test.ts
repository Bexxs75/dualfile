import { describe, expect, test } from "vitest";
import type { Entry, JobStatus } from "../ipc/types";
import { click } from "../state/selection";
import { activeTab, navigate } from "../state/tabs";
import { initialState, reducer, visibleEntries, type Action, type AppState } from "./state";

const entry = (name: string, kind: Entry["kind"] = "file", extra: Partial<Entry> = {}): Entry => ({
  name,
  kind,
  size: 1,
  modified: 0,
  mode: 0o644,
  hidden: name.startsWith("."),
  linkTarget: null,
  brokenLink: false,
  error: null,
  ...extra,
});

const run = (state: AppState, ...actions: Action[]): AppState => actions.reduce(reducer, state);
const ready = (): AppState =>
  run(initialState(), {
    type: "window",
    w: 0,
    update: (w) => navigate(w, "/a"),
    listing: { path: "/a", entries: [entry("b"), entry("a"), entry("dir", "dir"), entry(".h")] },
  });

describe("sichtbare Einträge", () => {
  test("„..“ zuerst, Ordner vor Dateien, Versteckte nur nach Strg+H, in der Wurzel kein „..“", () => {
    const state = ready();
    expect(visibleEntries(state, 0).map((e) => e.name)).toEqual(["..", "dir", "a", "b"]);
    expect(visibleEntries(run(state, { type: "toggleHidden" }), 0).map((e) => e.name)).toEqual(["..", "dir", ".h", "a", "b"]);
    const root = run(initialState(), { type: "listing", w: 0, path: "/", entries: [entry("x")] });
    expect(visibleEntries(root, 0).map((e) => e.name)).toEqual(["x"]);
  });

  test("solange ein neuer Ordner lädt, ist die Liste leer und die Markierung bleibt unberührt", () => {
    let state = ready();
    state = run(state, { type: "selection", w: 0, update: (s, e) => click(s, e, 2) });
    state = run(state, { type: "window", w: 0, update: (w) => navigate(w, "/b") });
    expect(visibleEntries(state, 0)).toEqual([]);
    state = run(state, { type: "window", w: 0, update: (w) => w });
    expect(activeTab(state.windows[0]).path).toBe("/b");
  });

  test("Ergebnis für einen veralteten Pfad wird verworfen", () => {
    const state = run(ready(), { type: "listing", w: 0, path: "/alt", entries: [entry("zzz")] });
    expect(visibleEntries(state, 0).map((e) => e.name)).not.toContain("zzz");
  });
});

describe("Auswahl nach dem Neuladen", () => {
  test("Markierung und Fokus bleiben über den Namen, Verschwundenes fällt heraus", () => {
    let state = ready();
    state = run(state, { type: "selection", w: 0, update: (s, e) => click(s, e, 3) });
    const marked = [...activeTab(state.windows[0]).selection.selected];
    expect(marked).toHaveLength(1);
    state = run(state, { type: "listing", w: 0, path: "/a", entries: [entry("neu"), entry("b"), entry("a")] });
    expect([...activeTab(state.windows[0]).selection.selected]).toEqual(marked);
    state = run(state, { type: "listing", w: 0, path: "/a", entries: [entry("neu")] });
    expect(activeTab(state.windows[0]).selection.selected.size).toBe(0);
  });

  test("Fokus springt nach dem Laden auf den vorgemerkten Namen", () => {
    const state = run(ready(), { type: "pendingFocus", w: 0, name: "b" }, { type: "listing", w: 0, path: "/a", entries: [entry("b"), entry("a")] });
    expect(visibleEntries(state, 0)[activeTab(state.windows[0]).selection.focus]?.name).toBe("b");
    expect(state.pendingFocus[0]).toBeNull();
  });

  test("Strg+H aus: unsichtbare Einträge sind nicht mehr markiert (nichts Verstecktes wird bearbeitet)", () => {
    let state = run(ready(), { type: "toggleHidden" });
    state = run(state, { type: "selection", w: 0, update: (s, e) => click(s, e, 2) });
    expect([...activeTab(state.windows[0]).selection.selected]).toEqual([".h"]);
    state = run(state, { type: "toggleHidden" });
    expect(activeTab(state.windows[0]).selection.selected.size).toBe(0);
  });
});

describe("Aufträge und Dialoge", () => {
  const job = (id: number, state: JobStatus["state"]): JobStatus => ({
    id, name: "x", state, filesDone: 0, bytesDone: 0, bytesTotal: 0, bytesPerSecond: 0, etaSeconds: null, queuePosition: null, lane: null, failed: [], skipped: [], warnings: [],
  });
  test("ein beendeter Auftrag entfernt seine offenen Konflikte", () => {
    const conflict = { jobId: 3, sourcePath: "/a/x", destinationPath: "/b/x", source: entry("x"), destination: entry("x") };
    let state = run(ready(), { type: "job", event: { type: "job", status: job(3, "running") } }, { type: "conflictAdd", conflict });
    expect(state.conflicts).toHaveLength(1);
    state = run(state, { type: "job", event: { type: "job", status: job(3, "cancelled") } });
    expect(state.conflicts).toHaveLength(0);
  });

  test("Rückgängig-Stapel und Meldungen", () => {
    let state = run(ready(), { type: "undoState", patch: { history: [{ id: 1, time: 0, kind: "trash", label: "undoLabel.trash", undone: false }] } }, { type: "toastAdd", text: "x", kind: "info" });
    expect(state.history).toHaveLength(1);
    expect(state.toasts[0]?.id).toBe(1);
    state = run(state, { type: "undoState", patch: { history: [] } }, { type: "toastRemove", id: 1 });
    expect(state.history).toHaveLength(0);
    expect(state.toasts).toHaveLength(0);
  });
});

test.each(["permissions", "uid", "gid"])("hidden %s sorting falls back to name ascending and invalidates cache", key => {
  let state = ready();
  const entries = [entry("a", "file", { mode: 1, uid: 1, gid: 1 }), entry("b", "file", { mode: 2, uid: 2, gid: 2 })];
  state = run(state, { type: "window", w: 0,
    update: win => ({ ...win, tabs: win.tabs.map(tab => ({ ...tab, sort: `${key}:desc` })) }),
    listing: { path: "/a", entries } });
  state = { ...state, settings: { ...state.settings, showOwnerColumns: true } };
  expect(visibleEntries(state, 0).map(e => e.name)).toEqual(["..", "b", "a"]);
  state = { ...state, settings: { ...state.settings, showOwnerColumns: false } };
  expect(visibleEntries(state, 0).map(e => e.name)).toEqual(["..", "a", "b"]);
  state = { ...state, settings: { ...state.settings, showOwnerColumns: true } };
  expect(visibleEntries(state, 0).map(e => e.name)).toEqual(["..", "b", "a"]);
});

test("toggling owner columns keeps the cursor on the same entry when the hidden sort reorders the list", () => {
  let state = ready();
  const entries = [entry("a", "file", { uid: 1 }), entry("b", "file", { uid: 2 })];
  state = run(state, { type: "window", w: 0,
    update: win => ({ ...win, tabs: win.tabs.map(tab => ({ ...tab, sort: "uid:desc" })) }),
    listing: { path: "/a", entries } });
  state = run(state, { type: "settings", update: s => ({ ...s, showOwnerColumns: true }) });
  expect(visibleEntries(state, 0).map(e => e.name)).toEqual(["..", "b", "a"]);
  state = run(state, { type: "selection", w: 0, update: (sel, e) => click(sel, e, 1) });
  state = run(state, { type: "settings", update: s => ({ ...s, showOwnerColumns: false }) });
  const focused = () => visibleEntries(state, 0)[activeTab(state.windows[0]).selection.focus]?.name;
  expect(focused()).toBe("b");
  expect(activeTab(state.windows[0]).selection.anchorName).toBe("b");
  state = run(state, { type: "settings", update: s => ({ ...s, showOwnerColumns: true }) });
  expect(focused()).toBe("b");
});
