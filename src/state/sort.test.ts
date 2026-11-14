import { describe, expect, it } from "vitest";
import type { Entry } from "../ipc/types";
import { formatSort, naturalCompare, parseSort, sortEntries, toggleSort } from "./sort";

function entry(name: string, extra: Partial<Entry> = {}): Entry {
  return {
    name,
    kind: "file",
    size: 0,
    modified: 0,
    mode: null,
    hidden: false,
    linkTarget: null,
    brokenLink: false,
    error: null,
    ...extra,
  };
}
const names = (list: Entry[]) => list.map((e) => e.name);

describe("naturalCompare", () => {
  it("sortiert Datei2 vor Datei10", () => {
    expect(naturalCompare("Datei2", "Datei10")).toBeLessThan(0);
    expect(naturalCompare("Datei10", "Datei2")).toBeGreaterThan(0);
  });
  it("ignoriert Groß-/Kleinschreibung mit stabilem Tiebreak", () => {
    expect(naturalCompare("abc", "ABD")).toBeLessThan(0);
    expect(naturalCompare("a", "A")).not.toBe(0);
    expect(naturalCompare("a", "A")).toBe(-naturalCompare("A", "a"));
    expect(naturalCompare("x", "x")).toBe(0);
  });
  it("ordnet Umlaute bei ihrem Grundbuchstaben ein", () => {
    expect(naturalCompare("Äpfel", "Birne")).toBeLessThan(0);
    expect(naturalCompare("Zebra", "Österreich")).toBeGreaterThan(0);
    expect(names(sortEntries([entry("Zug"), entry("Übung"), entry("Apfel")], "name"))).toEqual([
      "Apfel",
      "Übung",
      "Zug",
    ]);
  });
});

describe("sortEntries", () => {
  const mixed = [
    entry("b.txt", { size: 5, modified: 300 }),
    entry("zeta", { kind: "dir", modified: 100 }),
    entry("a.md", { size: 50, modified: 100 }),
    entry("alpha", { kind: "dir", modified: 200 }),
  ];
  it("setzt Ordner bei aufsteigender Sortierung zuerst", () => {
    expect(names(sortEntries(mixed, "name", "asc"))).toEqual(["alpha", "zeta", "a.md", "b.txt"]);
  });
  it("setzt Ordner auch bei absteigender Sortierung zuerst", () => {
    expect(names(sortEntries(mixed, "name", "desc"))).toEqual(["zeta", "alpha", "b.txt", "a.md"]);
  });
  it("hält .. immer ganz oben", () => {
    const withUp = [entry("a"), entry("..", { kind: "dir" }), entry("d", { kind: "dir" })];
    expect(names(sortEntries(withUp, "name", "desc"))[0]).toBe("..");
  });
  it("sortiert nach Typ (Endung kleingeschrieben)", () => {
    const list = [entry("x.TXT"), entry("y.md"), entry("z.txt"), entry("README")];
    expect(names(sortEntries(list, "type"))).toEqual(["README", "y.md", "x.TXT", "z.txt"]);
  });
  it("sortiert nach Größe, Ordner bleiben vorn und nach Name geordnet", () => {
    expect(names(sortEntries(mixed, "size", "desc"))).toEqual(["alpha", "zeta", "a.md", "b.txt"]);
    expect(names(sortEntries(mixed, "size", "asc"))).toEqual(["alpha", "zeta", "b.txt", "a.md"]);
  });
  it("sortiert nach Datum, unbekanntes Datum zuerst", () => {
    const list = [entry("neu", { modified: 900 }), entry("unbekannt", { modified: null }), entry("alt", { modified: 1 })];
    expect(names(sortEntries(list, "modified", "asc"))).toEqual(["unbekannt", "alt", "neu"]);
    expect(names(sortEntries(list, "modified", "desc"))).toEqual(["neu", "alt", "unbekannt"]);
  });
  it("ist stabil bei Gleichstand und verändert die Eingabe nicht", () => {
    const list = [entry("c", { size: 1 }), entry("a", { size: 1 }), entry("b", { size: 1 })];
    const before = names(list);
    expect(names(sortEntries(list, "size"))).toEqual(["c", "a", "b"]);
    expect(names(sortEntries(list, "size", "desc"))).toEqual(["c", "a", "b"]);
    expect(names(list)).toEqual(before);
  });
});

describe("parseSort / formatSort", () => {
  it("macht einen Roundtrip für alle Kombinationen", () => {
    for (const key of ["name", "size", "type", "modified"] as const) {
      for (const direction of ["asc", "desc"] as const) {
        expect(parseSort(formatSort({ key, direction }))).toEqual({ key, direction });
      }
    }
  });
  it("kürzt die Standardsortierung auf name", () => {
    expect(formatSort({ key: "name", direction: "asc" })).toBe("name");
    expect(parseSort("name")).toEqual({ key: "name", direction: "asc" });
    expect(parseSort("size:asc")).toEqual({ key: "size", direction: "asc" });
  });
  it("fällt bei ungültiger Eingabe auf die Standardsortierung zurück", () => {
    for (const bad of ["", "color", "name:up", "size:desc:x", "NAME"]) {
      expect(parseSort(bad)).toEqual({ key: "name", direction: "asc" });
    }
  });
  it("toggleSort kehrt um oder wechselt die Spalte", () => {
    expect(toggleSort({ key: "name", direction: "asc" }, "name")).toEqual({ key: "name", direction: "desc" });
    expect(toggleSort({ key: "name", direction: "desc" }, "size")).toEqual({ key: "size", direction: "asc" });
  });
});
