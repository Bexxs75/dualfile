import { describe, expect, it } from "vitest";
import type { Entry } from "../ipc/types";
import { filterEntries, matchPattern } from "./filter";

const e = (name: string, hidden = false): Entry => ({
  name,
  kind: "file",
  size: 1,
  modified: 0,
  mode: null,
  hidden,
  linkTarget: null,
  brokenLink: false,
  error: null,
});
const list = [e(".."), e("Bericht.txt"), e("bild01.png"), e("bild02.jpg"), e("Notizen.md"), e(".config", true)];
const names = (xs: Entry[]) => xs.map((x) => x.name);

describe("filterEntries", () => {
  it("findet Teilstrings ohne Groß-/Kleinschreibung", () => {
    expect(names(filterEntries(list, "BILD", false))).toEqual(["..", "bild01.png", "bild02.jpg"]);
    expect(names(filterEntries(list, "zen", false))).toEqual(["..", "Notizen.md"]);
  });
  it("unterstützt * und ? über den ganzen Namen", () => {
    expect(names(filterEntries(list, "*.png", false))).toEqual(["..", "bild01.png"]);
    expect(names(filterEntries(list, "bild0?.*", false))).toEqual(["..", "bild01.png", "bild02.jpg"]);
    expect(names(filterEntries(list, "bild", false))).toContain("bild01.png");
    expect(names(filterEntries(list, "bild?", false))).not.toContain("bild01.png");
  });
  it("unterstützt mehrere Muster mit Semikolon", () => {
    expect(names(filterEntries(list, "*.png;*.md", false))).toEqual(["..", "bild01.png", "Notizen.md"]);
    expect(names(filterEntries(list, "bericht;notiz", false))).toEqual(["..", "Bericht.txt", "Notizen.md"]);
  });
  it("blendet versteckte Einträge nur bei showHidden ein", () => {
    expect(names(filterEntries(list, "", false))).not.toContain(".config");
    expect(names(filterEntries(list, "", true))).toContain(".config");
    expect(names(filterEntries(list, "conf", true))).toEqual(["..", ".config"]);
  });
  it("lässt bei leerem Filter alle sichtbaren Einträge durch", () => {
    expect(names(filterEntries(list, "", false))).toEqual(["..", "Bericht.txt", "bild01.png", "bild02.jpg", "Notizen.md"]);
    expect(names(filterEntries(list, "  ;  ", false))).toHaveLength(5);
  });
  it("lässt .. auch bei nicht passendem Filter stehen", () => {
    expect(names(filterEntries(list, "gibtesnicht", false))).toEqual([".."]);
  });
});

describe("matchPattern", () => {
  it("ohne Platzhalter muss der Name genau passen", () => {
    expect(matchPattern("a.txt", "A.TXT")).toBe(true);
    expect(matchPattern("a.txt", "a")).toBe(false);
  });
  it("behandelt Sonderzeichen in Mustern wörtlich", () => {
    expect(matchPattern("a+b(1).txt", "a+b(1).*")).toBe(true);
    expect(matchPattern("axb", "a.b")).toBe(false);
  });
  it("kennt mehrere Muster und leere Muster", () => {
    expect(matchPattern("x.md", "*.txt; *.md")).toBe(true);
    expect(matchPattern("x.md", "")).toBe(false);
    expect(matchPattern("x.md", ";;")).toBe(false);
  });
  it("hängt bei langen Mustern nicht", () => {
    expect(matchPattern("a".repeat(5000), "*a*a*a*a*a*a*a*b")).toBe(false);
  });
});
