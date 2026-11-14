import { describe, expect, it } from "vitest";
import type { Bookmark } from "../ipc/types";
import { byShortcut, isValidColor, mirrorTarget, move, pin, rename, setColor, unpin } from "./bookmarks";

function must(result: ReturnType<typeof pin>): Bookmark[] {
  if (!result.ok) throw new Error(`unerwartet: ${result.error}`);
  return result.bookmarks;
}
function three(): Bookmark[] {
  return must(pin(must(pin(must(pin([], "/a")), "/b")), "/c"));
}
const paths = (b: Bookmark[]) => b.map((x) => x.path);
const orders = (b: Bookmark[]) => b.map((x) => x.order);

describe("pin / unpin", () => {
  it("heftet an, vorbelegt Name und Farbe", () => {
    const list = must(pin([], "/home/a/Projekte/"));
    expect(list).toEqual([{ name: "Projekte", path: "/home/a/Projekte", color: "#4f8cff", order: 0 }]);
  });
  it("heftet denselben Pfad nicht doppelt an", () => {
    const list = must(pin(three(), "/b/"));
    expect(paths(list)).toEqual(["/a", "/b", "/c"]);
  });
  it("lehnt leeren Namen und ungültige Farbe beim Anheften ab", () => {
    expect(pin([], "/a", "  ")).toEqual({ ok: false, error: "emptyName" });
    expect(pin([], "/a", "x", "red")).toEqual({ ok: false, error: "invalidColor" });
  });
  it("löst ein Lesezeichen und nummeriert lückenlos neu", () => {
    const list = unpin(three(), "/b");
    expect(paths(list)).toEqual(["/a", "/c"]);
    expect(orders(list)).toEqual([0, 1]);
  });
  it("Lösen eines unbekannten Pfads ändert nichts", () => {
    expect(paths(unpin(three(), "/zzz"))).toEqual(["/a", "/b", "/c"]);
  });
});

describe("rename / Farbe", () => {
  it("benennt um und lehnt leeren Namen ab", () => {
    expect(must(rename(three(), "/b", " Neu "))[1]!.name).toBe("Neu");
    expect(rename(three(), "/b", "")).toEqual({ ok: false, error: "emptyName" });
    expect(rename(three(), "/zzz", "x")).toEqual({ ok: false, error: "notFound" });
  });
  it("akzeptiert nur #rrggbb", () => {
    expect(must(setColor(three(), "/a", "#AABBCC"))[0]!.color).toBe("#aabbcc");
    for (const bad of ["red", "#fff", "#12345", "#1234567", "aabbcc", "#gggggg", "#aabbcc; x", ""]) {
      expect(setColor(three(), "/a", bad)).toEqual({ ok: false, error: "invalidColor" });
      expect(isValidColor(bad)).toBe(false);
    }
  });
});

describe("Reihenfolge", () => {
  it("move ordnet um und hält order lückenlos 0..n-1", () => {
    const list = must(move(three(), 0, 2));
    expect(paths(list)).toEqual(["/b", "/c", "/a"]);
    expect(orders(list)).toEqual([0, 1, 2]);
    const back = must(move(list, 2, 0));
    expect(paths(back)).toEqual(["/a", "/b", "/c"]);
    expect(orders(back)).toEqual([0, 1, 2]);
  });
  it("move von unsortierter Eingabe mit Lücken liefert trotzdem 0..n-1", () => {
    const gappy: Bookmark[] = [
      { name: "x", path: "/x", color: "#000000", order: 10 },
      { name: "y", path: "/y", color: "#000000", order: 4 },
    ];
    expect(orders(must(move(gappy, 0, 1)))).toEqual([0, 1]);
    expect(paths(must(move(gappy, 0, 0)))).toEqual(["/y", "/x"]);
  });
  it("lehnt ungültige Indizes ab", () => {
    expect(move(three(), -1, 1)).toEqual({ ok: false, error: "badIndex" });
    expect(move(three(), 0, 3)).toEqual({ ok: false, error: "badIndex" });
  });
});

describe("Alt+1…9", () => {
  it("liefert das n-te Lesezeichen in Anzeigereihenfolge", () => {
    const list = must(move(three(), 2, 0)); // c, a, b
    expect(byShortcut(list, 1)?.path).toBe("/c");
    expect(byShortcut(list, 3)?.path).toBe("/b");
  });
  it("liefert null außerhalb von 1…9 oder ohne Lesezeichen", () => {
    expect(byShortcut(three(), 4)).toBeNull();
    expect(byShortcut(three(), 0)).toBeNull();
    expect(byShortcut(three(), 10)).toBeNull();
    expect(byShortcut([], 1)).toBeNull();
  });
});

describe("Spiegeln", () => {
  it("gibt bei aktivem Spiegeln das Ziel für das andere Fenster zurück", () => {
    expect(mirrorTarget(three(), true, "/b")).toBe("/b");
    expect(mirrorTarget(three(), true, "/b/")).toBe("/b");
  });
  it("tut nichts, wenn Spiegeln aus ist", () => {
    expect(mirrorTarget(three(), false, "/b")).toBeNull();
  });
  it("zieht bei gewöhnlicher Navigation (kein Lesezeichen) nicht mit", () => {
    expect(mirrorTarget(three(), true, "/b/unter")).toBeNull();
    expect(mirrorTarget([], true, "/b")).toBeNull();
  });
});
