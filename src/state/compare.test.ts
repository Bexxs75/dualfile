import { describe, expect, test } from "vitest";
import type { DiffHunk, SyncAction } from "../ipc/types";
import {
  allowedSyncModes,
  buildDiffRows,
  canDiffEntry,
  checkedRels,
  differenceHunks,
  isDestructive,
  neededDirs,
  newCompareSession,
  stepDifference,
} from "./compare";

const action = (kind: SyncAction["kind"], rel: string): SyncAction => ({ index: 0, kind, rel, entryKind: kind === "makeDir" ? "dir" : "file", size: 1 });

describe("allowedSyncModes", () => {
  test("lokal: alle Richtungen", () => {
    expect(allowedSyncModes("/a", "/b")).toEqual(["leftToRight", "rightToLeft", "mirror"]);
  });
  test("ein Archiv ist nie Ziel", () => {
    expect(allowedSyncModes("archive:/a.zip!/", "/b")).toEqual(["leftToRight"]);
    expect(allowedSyncModes("/a", "archive:/b.zip!/")).toEqual(["rightToLeft"]);
    expect(allowedSyncModes("archive:/a.zip!/", "archive:/b.zip!/")).toEqual([]);
  });
});

describe("Zeilen der Textansicht", () => {
  const hunks: DiffHunk[] = [
    { kind: "equal", leftStart: 0, left: ["a", "b"], rightStart: 0, right: ["a", "b"] },
    { kind: "delete", leftStart: 2, left: ["x"], rightStart: 2, right: [] },
    { kind: "equal", leftStart: 3, left: ["c"], rightStart: 2, right: ["c"] },
    { kind: "insert", leftStart: 4, left: [], rightStart: 3, right: ["y", "z"] },
    { kind: "replace", leftStart: 4, left: ["p", "q", "r"], rightStart: 5, right: ["s"] },
  ];
  const rows = buildDiffRows(hunks);

  test("Zeilennummern zählen ab 1, Lücken bleiben leer", () => {
    expect(rows[0]).toMatchObject({ kind: "equal", leftNo: 1, rightNo: 1, left: "a", right: "a" });
    expect(rows[2]).toMatchObject({ kind: "delete", leftNo: 3, left: "x", rightNo: null, right: null });
    expect(rows[4]).toMatchObject({ kind: "insert", leftNo: null, rightNo: 4, right: "y" });
    expect(rows[5]).toMatchObject({ kind: "insert", rightNo: 5, right: "z" });
  });
  test("ungleich lange Ersetzungen werden mit Lücken aufgefüllt", () => {
    const replace = rows.filter((row) => row.kind === "replace");
    expect(replace).toHaveLength(3);
    expect(replace.map((row) => row.left)).toEqual(["p", "q", "r"]);
    expect(replace.map((row) => row.right)).toEqual(["s", null, null]);
  });
  test("Unterschiede: nur Abschnitte, die nicht gleich sind", () => {
    expect(differenceHunks(hunks)).toEqual([1, 3, 4]);
  });
  test("Springen bleibt an den Enden stehen; ohne Unterschied gibt es keine Position", () => {
    expect(stepDifference(0, -1, 3)).toBe(0);
    expect(stepDifference(0, 1, 3)).toBe(1);
    expect(stepDifference(2, 1, 3)).toBe(2);
    expect(stepDifference(-1, 1, 0)).toBe(-1);
  });
});

describe("Sync-Auswahl", () => {
  const actions = [action("makeDir", "d"), action("copy", "d/a"), action("copy", "d/b"), action("trash", "x"), action("replace", "r")];
  test("rot markiert sind nur Überschreiben und Papierkorb", () => {
    expect(actions.map(isDestructive)).toEqual([false, false, false, true, true]);
  });
  test("ein Ordner ist „benötigt“, solange eine angehakte Aktion darin liegt", () => {
    expect([...neededDirs(actions, new Set())]).toEqual(["d"]);
    expect([...neededDirs(actions, new Set(["d/a"]))]).toEqual(["d"]);
    expect([...neededDirs(actions, new Set(["d/a", "d/b"]))]).toEqual([]);
  });
  test("checkedRels lässt Abgewähltes weg", () => {
    expect(checkedRels(actions, new Set(["d", "x"]))).toEqual(["d/a", "d/b", "r"]);
  });
});

describe("Sitzung", () => {
  test("neu: Größe+Zeit, mit Unterordnern, ohne Filter und Auswahl", () => {
    const s = newCompareSession("/l", "/r");
    expect(s).toMatchObject({ phase: "setup", options: { mode: "sizeTime", recursive: true }, jobId: null, sync: null });
    expect(s.filter).toEqual({ states: [], text: "" });
    expect(s.selected.size).toBe(0);
  });
  test("Textvergleich nur für Datei gegen Datei", () => {
    const side = (kind: "file" | "dir") => ({ kind, size: 1, modified: 1, linkTarget: null });
    const entry = (l: "file" | "dir" | null, r: "file" | "dir" | null, state: "same" | "unknown" = "same") => ({
      rel: "x", state, left: l ? side(l) : null, right: r ? side(r) : null, note: null, listed: false,
    });
    expect(canDiffEntry(entry("file", "file"))).toBe(true);
    expect(canDiffEntry(entry("file", null))).toBe(false);
    expect(canDiffEntry(entry("dir", "file"))).toBe(false);
    expect(canDiffEntry(entry("file", "file", "unknown"))).toBe(false);
  });
});
