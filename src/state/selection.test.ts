import { describe, expect, it } from "vitest";
import type { Entry } from "../ipc/types";
import {
  clear,
  click,
  createSelection,
  ctrlClick,
  deselectByPattern,
  invert,
  moveFocus,
  reconcile,
  selectAll,
  selectByPattern,
  shiftClick,
  summary,
  toggleAndAdvance,
} from "./selection";

const e = (name: string, extra: Partial<Entry> = {}): Entry => ({
  name,
  kind: "file",
  size: 10,
  modified: 0,
  mode: null,
  hidden: false,
  linkTarget: null,
  brokenLink: false,
  error: null,
  ...extra,
});
// Index: 0 .., 1 a.txt, 2 b.txt, 3 c.md, 4 d.md, 5 e.png, 6 sub (Ordner)
const entries: Entry[] = [
  e("..", { kind: "dir", size: 0 }),
  e("a.txt"),
  e("b.txt"),
  e("c.md"),
  e("d.md"),
  e("e.png"),
  e("sub", { kind: "dir", size: 4096 }),
];
const sel = (s: { selected: ReadonlySet<string> }) => [...s.selected].sort();

describe("Klick-Markierung", () => {
  it("Klick markiert nur den angeklickten Eintrag und setzt Fokus und Anker", () => {
    const s = click(click(createSelection(), entries, 1), entries, 3);
    expect(sel(s)).toEqual(["c.md"]);
    expect(s.focus).toBe(3);
    expect(s.anchor).toBe(3);
  });
  it("Strg-Klick schaltet einzelne Einträge um", () => {
    let s = click(createSelection(), entries, 1);
    s = ctrlClick(s, entries, 3);
    expect(sel(s)).toEqual(["a.txt", "c.md"]);
    s = ctrlClick(s, entries, 1);
    expect(sel(s)).toEqual(["c.md"]);
  });
  it("Umschalt-Klick markiert den Bereich ab Anker abwärts", () => {
    const s = shiftClick(click(createSelection(), entries, 2), entries, 4);
    expect(sel(s)).toEqual(["b.txt", "c.md", "d.md"]);
    expect(s.anchor).toBe(2);
  });
  it("Umschalt-Klick markiert den Bereich ab Anker aufwärts", () => {
    const s = shiftClick(click(createSelection(), entries, 4), entries, 2);
    expect(sel(s)).toEqual(["b.txt", "c.md", "d.md"]);
  });
  it("Umschalt-Klick auf kleineren Bereich nimmt Überstand wieder heraus", () => {
    let s = shiftClick(click(createSelection(), entries, 1), entries, 5);
    s = shiftClick(s, entries, 3);
    expect(sel(s)).toEqual(["a.txt", "b.txt", "c.md"]);
  });
  it("Umschalt-Klick behält Strg-Markierungen außerhalb des Bereichs", () => {
    let s = ctrlClick(click(createSelection(), entries, 5), entries, 1);
    s = ctrlClick(s, entries, 3);
    s = shiftClick(s, entries, 4);
    expect(sel(s)).toEqual(["a.txt", "c.md", "d.md", "e.png"]);
  });
});

describe("Einfg/Leertaste", () => {
  it("markiert und geht eine Zeile weiter", () => {
    let s = click(createSelection(), entries, 1);
    s = clear(s);
    s = toggleAndAdvance(s, entries);
    expect(sel(s)).toEqual(["a.txt"]);
    expect(s.focus).toBe(2);
    s = toggleAndAdvance(s, entries);
    expect(sel(s)).toEqual(["a.txt", "b.txt"]);
  });
  it("hebt eine Markierung beim zweiten Durchlauf wieder auf", () => {
    let s = toggleAndAdvance(clear(click(createSelection(), entries, 1)), entries);
    expect(sel(s)).toEqual(["a.txt"]);
    s = moveFocus(s, entries, -1);
    s = toggleAndAdvance(s, entries);
    expect(sel(s)).toEqual([]);
  });
  it("bleibt am Listenende stehen und markiert trotzdem", () => {
    const s = toggleAndAdvance(click(createSelection(), entries, 6), entries);
    expect(s.focus).toBe(6);
    expect(sel(s)).toEqual([]);
    const once = toggleAndAdvance(clear(click(createSelection(), entries, 5)), entries);
    expect(sel(once)).toEqual(["e.png"]);
  });
});

describe("Fokus bewegen", () => {
  it("Pos1/Ende springen an Anfang/Ende", () => {
    const s = click(createSelection(), entries, 3);
    expect(moveFocus(s, entries, "home").focus).toBe(0);
    expect(moveFocus(s, entries, "end").focus).toBe(6);
  });
  it("Bild auf/ab bewegt um die Seitengröße und begrenzt an den Rändern", () => {
    const s = click(createSelection(), entries, 3);
    expect(moveFocus(s, entries, "pageDown", 2).focus).toBe(5);
    expect(moveFocus(s, entries, "pageDown", 50).focus).toBe(6);
    expect(moveFocus(s, entries, "pageUp", 2).focus).toBe(1);
    expect(moveFocus(s, entries, "pageUp", 50).focus).toBe(0);
  });
  it("Pfeil bewegt um eins und begrenzt", () => {
    const s = click(createSelection(), entries, 0);
    expect(moveFocus(s, entries, -1).focus).toBe(0);
    expect(moveFocus(s, entries, 1).focus).toBe(1);
  });
  it("Umschalt+Pfeil markiert Bereich und nimmt beim Zurückgehen wieder heraus", () => {
    let s = click(createSelection(), entries, 2);
    s = moveFocus(s, entries, 1, 10, true);
    s = moveFocus(s, entries, 1, 10, true);
    expect(sel(s)).toEqual(["b.txt", "c.md", "d.md"]);
    s = moveFocus(s, entries, -1, 10, true);
    expect(sel(s)).toEqual(["b.txt", "c.md"]);
  });
  it("Umschalt+Pos1 markiert bis zum Anfang ohne ..", () => {
    const s = moveFocus(click(createSelection(), entries, 3), entries, "home", 10, true);
    expect(sel(s)).toEqual(["a.txt", "b.txt", "c.md"]);
  });
  it("leere Liste ändert nichts", () => {
    const s = createSelection();
    expect(moveFocus(s, [], "end")).toBe(s);
  });
});

describe("Muster, Umkehren, Alles", () => {
  it("+ markiert nach Muster zusätzlich", () => {
    const s = selectByPattern(click(createSelection(), entries, 5), entries, "*.md;*.txt");
    expect(sel(s)).toEqual(["a.txt", "b.txt", "c.md", "d.md", "e.png"]);
  });
  it("- hebt nach Muster auf", () => {
    const s = deselectByPattern(selectAll(createSelection(), entries), entries, "*.md");
    expect(sel(s)).toEqual(["a.txt", "b.txt", "e.png", "sub"]);
  });
  it("* kehrt die Markierung um", () => {
    const s = invert(click(createSelection(), entries, 1), entries);
    expect(sel(s)).toEqual(["b.txt", "c.md", "d.md", "e.png", "sub"]);
  });
  it("Strg+A markiert alles außer ..", () => {
    expect(sel(selectAll(createSelection(), entries))).toEqual(["a.txt", "b.txt", "c.md", "d.md", "e.png", "sub"]);
  });
  it("clear hebt alles auf", () => {
    expect(sel(clear(selectAll(createSelection(), entries)))).toEqual([]);
  });
});

describe("„..“ ist nie markierbar", () => {
  it("Klick, Strg-Klick, Muster, Umkehren und Alles lassen .. aus", () => {
    expect(sel(click(createSelection(), entries, 0))).toEqual([]);
    expect(sel(ctrlClick(createSelection(), entries, 0))).toEqual([]);
    expect(sel(selectByPattern(createSelection(), entries, "*"))).not.toContain("..");
    expect(sel(invert(createSelection(), entries))).not.toContain("..");
    expect(sel(selectAll(createSelection(), entries))).not.toContain("..");
  });
  it("Bereichsmarkierung über .. hinweg und Einfg auf .. lassen .. aus", () => {
    expect(sel(shiftClick(click(createSelection(), entries, 2), entries, 0))).toEqual(["a.txt", "b.txt"]);
    const s = toggleAndAdvance(click(createSelection(), entries, 0), entries);
    expect(sel(s)).toEqual([]);
    expect(s.focus).toBe(1);
  });
});

describe("reconcile", () => {
  it("behält Markierung und Fokus beim Neuladen (auch bei geänderter Reihenfolge)", () => {
    let s = ctrlClick(click(createSelection(), entries, 3), entries, 5);
    s = moveFocus(s, entries, -1);
    const reordered = [entries[0]!, entries[6]!, entries[5]!, entries[4]!, entries[3]!, entries[2]!, entries[1]!];
    const r = reconcile(s, reordered);
    expect(sel(r)).toEqual(["c.md", "e.png"]);
    expect(r.focusedName).toBe("d.md");
    expect(reordered[r.focus]!.name).toBe("d.md");
  });
  it("entfernt gelöschte Namen aus der Markierung", () => {
    const s = selectAll(createSelection(), entries);
    const r = reconcile(s, entries.filter((x) => x.name !== "b.txt" && x.name !== "sub"));
    expect(sel(r)).toEqual(["a.txt", "c.md", "d.md", "e.png"]);
  });
  it("setzt den Fokus bei gelöschtem Eintrag auf den nächsten Nachbarn", () => {
    const s = click(createSelection(), entries, 3);
    const r = reconcile(s, entries.filter((x) => x.name !== "c.md"));
    expect(r.focusedName).toBe("d.md");
  });
  it("setzt den Fokus bei gelöschtem letztem Eintrag auf den neuen letzten", () => {
    const s = click(createSelection(), entries, 6);
    const r = reconcile(s, entries.slice(0, 6));
    expect(r.focusedName).toBe("e.png");
  });
  it("kommt mit leerer Liste zurecht", () => {
    const r = reconcile(selectAll(createSelection(), entries), []);
    expect(r.focus).toBe(0);
    expect(r.focusedName).toBeNull();
    expect(sel(r)).toEqual([]);
  });
});

describe("summary", () => {
  it("zählt Ordner als markiert, summiert aber nur Dateigrößen", () => {
    const s = ctrlClick(ctrlClick(click(createSelection(), entries, 1), entries, 2), entries, 6);
    expect(summary(s, entries)).toEqual({ selectedCount: 3, totalCount: 6, selectedBytes: 20 });
  });
  it("ohne Markierung ist alles null außer der Gesamtzahl", () => {
    expect(summary(createSelection(), entries)).toEqual({ selectedCount: 0, totalCount: 6, selectedBytes: 0 });
  });
  it("ignoriert Markierungen auf nicht mehr vorhandene Namen", () => {
    const s = selectAll(createSelection(), entries);
    expect(summary(s, entries.slice(0, 3)).selectedCount).toBe(2);
  });
});
