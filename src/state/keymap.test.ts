import { describe, expect, it } from "vitest";
import { bookmarkNumber, type Command, type KeyboardEventLike, mapDiffKey, mapKey } from "./keymap";

const k = (key: string, mods: Partial<KeyboardEventLike> = {}): KeyboardEventLike => ({ key, ...mods });
const list = (key: string, mods: Partial<KeyboardEventLike> = {}) => mapKey(k(key, mods), "list");

describe("Listenbefehle", () => {
  it.each<[string, Command]>([
    ["Tab", "switchPane"],
    ["ArrowUp", "moveUp"],
    ["ArrowDown", "moveDown"],
    ["Home", "home"],
    ["End", "end"],
    ["PageUp", "pageUp"],
    ["PageDown", "pageDown"],
    ["Enter", "open"],
    ["Backspace", "goUp"],
    ["F2", "rename"],
    ["F3", "view"],
    ["F4", "edit"],
    ["F5", "copy"],
    ["F6", "move"],
    ["F7", "makeDir"],
    ["F8", "trash"],
    ["Delete", "trash"],
    ["Insert", "toggleSelection"],
    [" ", "toggleSelection"],
    ["+", "selectByPattern"],
    ["-", "deselectByPattern"],
    ["*", "invertSelection"],
    ["Escape", "escape"],
  ])("Taste %s ergibt %s", (key, expected) => {
    expect(list(key)).toBe(expected);
  });
});

describe("Mehrfach-Umbenennen (Strg+M)", () => {
  it("Strg+M ist in der Liste der Befehl multiRename, mit Umschalt/Alt/Meta nicht", () => {
    expect(list("m", { ctrlKey: true })).toBe("multiRename");
    expect(list("M", { ctrlKey: true })).toBe("multiRename");
    expect(list("m", { ctrlKey: true, shiftKey: true })).toBeNull();
    expect(list("m", { ctrlKey: true, altKey: true })).toBeNull();
    expect(list("m", { ctrlKey: true, metaKey: true })).toBeNull();
    expect(list("m")).toBeNull();
  });
  it("in Eingabefeldern und Dialogen gehört Strg+M nicht der Liste", () => {
    expect(mapKey(k("m", { ctrlKey: true }), "input")).toBeNull();
    expect(mapKey(k("m", { ctrlKey: true }), "dialog")).toBeNull();
  });
});

describe("Strg-Kombinationen", () => {
  it.each<[string, Command]>([
    ["a", "selectAll"],
    ["l", "pathInput"],
    ["r", "refresh"],
    ["h", "toggleHidden"],
    ["u", "swapPanes"],
    ["t", "newTab"],
    ["w", "closeTab"],
    ["z", "undo"],
    ["q", "quickView"],
    ["Tab", "nextTab"],
  ])("Strg+%s ergibt %s", (key, expected) => {
    expect(list(key, { ctrlKey: true })).toBe(expected);
  });
  it("erkennt Großbuchstaben (Feststelltaste) und Strg+Umschalt+Tab", () => {
    expect(list("A", { ctrlKey: true })).toBe("selectAll");
    expect(list("Tab", { ctrlKey: true, shiftKey: true })).toBe("prevTab");
  });
  it("Strg+Umschalt+P ist Packen, nur in der Liste", () => {
    expect(list("P", { ctrlKey: true, shiftKey: true })).toBe("pack");
    expect(list("p", { ctrlKey: true, shiftKey: true })).toBe("pack");
    expect(list("p", { ctrlKey: true })).toBeNull();
    expect(mapKey(k("P", { ctrlKey: true, shiftKey: true }), "input")).toBeNull();
    expect(mapKey(k("P", { ctrlKey: true, shiftKey: true }), "dialog")).toBeNull();
  });
  it("ordnet unbelegte Strg-Kombinationen nicht zu", () => {
    expect(list("x", { ctrlKey: true })).toBeNull();
    expect(list("F5", { ctrlKey: true })).toBeNull();
    expect(list("Delete", { ctrlKey: true })).toBeNull();
    expect(list("a", { ctrlKey: true, shiftKey: true })).toBeNull();
  });
});

describe("Alt+1…9 (Lesezeichen)", () => {
  it("liefert bookmark1…bookmark9 mit passender Nummer", () => {
    for (let n = 1; n <= 9; n++) {
      const command = list(String(n), { altKey: true });
      expect(command).toBe(`bookmark${n}`);
      expect(bookmarkNumber(command as Parameters<typeof bookmarkNumber>[0])).toBe(n);
    }
  });
  it("ignoriert Alt+0, Alt+Buchstaben und Alt+F4", () => {
    expect(list("0", { altKey: true })).toBeNull();
    expect(list("a", { altKey: true })).toBeNull();
    expect(list("F4", { altKey: true })).toBeNull();
  });
  it("eine Ziffer ohne Alt ist kein Befehl", () => {
    expect(list("1")).toBeNull();
  });
});

describe("Umschalt-Varianten", () => {
  it("Umschalt+F6 benennt um, Umschalt+F8 und Umschalt+Entf löschen endgültig", () => {
    expect(list("F6", { shiftKey: true })).toBe("rename");
    expect(list("F8", { shiftKey: true })).toBe("deletePermanent");
    expect(list("Delete", { shiftKey: true })).toBe("deletePermanent");
  });
  it("Umschalt+Pfeile/Pos1/Ende/Bild erweitern die Markierung", () => {
    expect(list("ArrowUp", { shiftKey: true })).toBe("extendUp");
    expect(list("ArrowDown", { shiftKey: true })).toBe("extendDown");
    expect(list("Home", { shiftKey: true })).toBe("extendHome");
    expect(list("End", { shiftKey: true })).toBe("extendEnd");
    expect(list("PageUp", { shiftKey: true })).toBe("extendPageUp");
    expect(list("PageDown", { shiftKey: true })).toBe("extendPageDown");
  });
  it("+ und * funktionieren auch mit Umschalt (Layout), Umschalt+Tab wechselt das Fenster", () => {
    expect(list("+", { shiftKey: true })).toBe("selectByPattern");
    expect(list("*", { shiftKey: true })).toBe("invertSelection");
    expect(list("Tab", { shiftKey: true })).toBe("switchPane");
  });
  it("unbelegte Umschalt-Kombinationen ergeben null (kein stilles Kopieren/Löschen)", () => {
    expect(list("F5", { shiftKey: true })).toBeNull();
    expect(list("F7", { shiftKey: true })).toBeNull();
    expect(list("Enter", { shiftKey: true })).toBeNull();
  });
  it("Entf und F8 ohne Umschalt sind nie endgültiges Löschen", () => {
    expect(list("Delete")).not.toBe("deletePermanent");
    expect(list("F8")).not.toBe("deletePermanent");
  });
});

describe("Kontext", () => {
  it("Eingabefeld blockiert Listenbefehle", () => {
    for (const key of ["ArrowUp", "ArrowDown", "Backspace", "Delete", " ", "+", "-", "*", "Enter", "F5", "F8", "Home", "Tab"]) {
      expect(mapKey(k(key), "input"), key).toBeNull();
    }
    expect(mapKey(k("a", { ctrlKey: true }), "input")).toBeNull();
    expect(mapKey(k("F8", { shiftKey: true }), "input")).toBeNull();
    expect(mapKey(k("1", { altKey: true }), "input")).toBeNull();
  });
  it("Dialog blockiert Listenbefehle", () => {
    for (const key of ["ArrowUp", "Delete", " ", "F5", "F8", "Enter", "Tab", "Backspace"]) {
      expect(mapKey(k(key), "dialog"), key).toBeNull();
    }
    expect(mapKey(k("F8", { shiftKey: true }), "dialog")).toBeNull();
    expect(mapKey(k("t", { ctrlKey: true }), "dialog")).toBeNull();
  });
  it("Esc bleibt in Eingabefeld und Dialog erhalten", () => {
    expect(mapKey(k("Escape"), "input")).toBe("escape");
    expect(mapKey(k("Escape"), "dialog")).toBe("escape");
  });
  it("Tab-Befehle (Strg+T/W/Tab) gelten im Eingabefeld, nicht im Dialog", () => {
    expect(mapKey(k("t", { ctrlKey: true }), "input")).toBe("newTab");
    expect(mapKey(k("Tab", { ctrlKey: true }), "input")).toBe("nextTab");
    expect(mapKey(k("t", { ctrlKey: true }), "dialog")).toBeNull();
  });
  it("Standardkontext ist list", () => {
    expect(mapKey(k("F5"))).toBe("copy");
  });
});

describe("Unbekanntes", () => {
  it("unbekannte Tasten ergeben null", () => {
    for (const key of ["a", "x", "F1", "F12", "Shift", "Control", "Alt", "ä", "Dead"]) {
      expect(list(key), key).toBeNull();
    }
  });
  it("Meta-Kombinationen gehören dem System", () => {
    expect(list("a", { metaKey: true })).toBeNull();
    expect(list("F5", { metaKey: true })).toBeNull();
  });
});

describe("Vergleichen (Phase 5c)", () => {
  it("Strg+Umschalt+D öffnet den Ordnervergleich, Strg+Umschalt+F den Textvergleich", () => {
    expect(list("D", { ctrlKey: true, shiftKey: true })).toBe("compareDirs");
    expect(list("d", { ctrlKey: true, shiftKey: true })).toBe("compareDirs");
    expect(list("F", { ctrlKey: true, shiftKey: true })).toBe("compareFiles");
  });
  it("Strg+D (anheften) und Strg+F (Filter) bleiben frei und ergeben keinen Befehl", () => {
    expect(list("d", { ctrlKey: true })).toBeNull();
    expect(list("f", { ctrlKey: true })).toBeNull();
  });
  it("in Eingabefeldern und Dialogen greifen sie nicht", () => {
    for (const context of ["input", "dialog"] as const) {
      expect(mapKey(k("D", { ctrlKey: true, shiftKey: true }), context)).toBeNull();
      expect(mapKey(k("F", { ctrlKey: true, shiftKey: true }), context)).toBeNull();
    }
  });
  it("Textvergleich-Ansicht: n/F3 nächster, p/Umschalt+F3 voriger Unterschied", () => {
    expect(mapDiffKey(k("n"))).toBe("nextDifference");
    expect(mapDiffKey(k("F3"))).toBe("nextDifference");
    expect(mapDiffKey(k("p"))).toBe("prevDifference");
    expect(mapDiffKey(k("F3", { shiftKey: true }))).toBe("prevDifference");
    expect(mapDiffKey(k("n", { ctrlKey: true }))).toBeNull();
    expect(mapDiffKey(k("N", { shiftKey: true }))).toBeNull();
    expect(mapDiffKey(k("x"))).toBeNull();
  });
  test("terminal toggle works via the physical backtick key on any layout", () => {
    expect(mapKey({ key: "`", ctrlKey: true }, "list")).toBe("terminal"); // US backtick
    expect(mapKey({ key: "^", code: "Backquote", ctrlKey: true }, "list")).toBe("terminal"); // German ^° key
    expect(mapKey({ key: "Dead", code: "Backquote", ctrlKey: true }, "list")).toBe("terminal"); // dead-key variant
    expect(mapKey({ key: "^", code: "Backquote", ctrlKey: true, shiftKey: true }, "list")).not.toBe("terminal");
  });
});
