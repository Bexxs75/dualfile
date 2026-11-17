import { describe, expect, it } from "vitest";
import { eventToSpec, mapKey, REBINDABLE, REBINDABLE_COMMANDS, type Command, type KeyboardEventLike } from "./keymap";

const k = (key: string, mods: Partial<KeyboardEventLike> = {}): KeyboardEventLike => ({ key, ...mods });

describe("Standardtasten ohne Overrides", () => {
  it.each<[KeyboardEventLike, Command]>([
    [k("F2"), "rename"], [k("F3"), "view"], [k("F4"), "edit"], [k("F5"), "copy"], [k("F6"), "move"],
    [k("F7"), "makeDir"], [k("F8"), "trash"], [k("F8", { shiftKey: true }), "deletePermanent"],
    [k("a", { ctrlKey: true }), "selectAll"], [k("A", { ctrlKey: true }), "selectAll"],
    [k("F", { ctrlKey: true, shiftKey: true }), "compareFiles"], [k("p", { ctrlKey: true, shiftKey: true }), "pack"],
    [k("ArrowUp"), "moveUp"], [k(" "), "toggleSelection"], [k("Enter"), "open"],
    [k("Delete"), "trash"], [k("Delete", { shiftKey: true }), "deletePermanent"], [k("F6", { shiftKey: true }), "rename"],
    [k("Tab", { ctrlKey: true }), "nextTab"], [k("3", { altKey: true }), "bookmark3"],
  ])("%j", (event, command) => {
    expect(mapKey(event, "list")).toBe(command);
    expect(mapKey(event, "list", {})).toBe(command);
  });

  it("jeder umbelegbare Befehl liegt auf seiner Standardtaste", () => {
    for (const command of REBINDABLE_COMMANDS) {
      const spec = REBINDABLE[command];
      if (!spec) { expect(mapKey(k(spec))).toBeNull(); continue; }
      const parts = spec.split("+");
      const key = parts[parts.length - 1];
      const event = k(key, { ctrlKey: parts.includes("Ctrl"), shiftKey: parts.includes("Shift") });
      expect(mapKey(event)).toBe(command);
    }
  });
});

describe("Overrides", () => {
  it("rename auf F9: F9 greift, F2 ist leer und nicht strukturell", () => {
    const b = { rename: "F9" };
    expect(mapKey(k("F9"), "list", b)).toBe("rename");
    expect(mapKey(k("F2"), "list", b)).toBeNull();
    expect(mapKey(k("F5"), "list", b)).toBe("copy");
  });
  it("Tausch copy/move", () => {
    const b = { copy: "F6", move: "F5" };
    expect(mapKey(k("F6"), "list", b)).toBe("copy");
    expect(mapKey(k("F5"), "list", b)).toBe("move");
  });
  it("Strg-Override und strukturelle Aliase bleiben", () => {
    const b = { trash: "Ctrl+Shift+G", rename: "F9" };
    expect(mapKey(k("g", { ctrlKey: true, shiftKey: true }), "list", b)).toBe("trash");
    expect(mapKey(k("F8"), "list", b)).toBeNull();
    expect(mapKey(k("Delete"), "list", b)).toBe("trash");
    expect(mapKey(k("F6", { shiftKey: true }), "list", b)).toBe("rename");
  });
  it("ungültige Override-Einträge werden ignoriert", () => {
    expect(mapKey(k("F2"), "list", { moveUp: "F2", bogus: "F2" })).toBe("rename");
  });
  it("Kontextfilter bleibt: nur Esc und Tab-Befehle außerhalb der Liste", () => {
    const b = { rename: "F9", newTab: "F10" };
    expect(mapKey(k("F9"), "input", b)).toBeNull();
    expect(mapKey(k("F9"), "dialog", b)).toBeNull();
    expect(mapKey(k("F10"), "input", b)).toBe("newTab");
    expect(mapKey(k("F10"), "dialog", b)).toBeNull();
    expect(mapKey(k("Escape"), "dialog", b)).toBe("escape");
  });
});

describe("eventToSpec", () => {
  it.each<[KeyboardEventLike, string]>([
    [k("F3"), "F3"], [k("F6", { shiftKey: true }), "Shift+F6"], [k("a", { ctrlKey: true }), "Ctrl+A"],
    [k("f", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+F"], [k("F", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+F"],
  ])("%j", (event, spec) => expect(eventToSpec(event)).toBe(spec));
  it.each<KeyboardEventLike>([
    k("ArrowUp"), k(" "), k("Enter"), k("Delete"), k("a"), k("F4", { altKey: true }), k("a", { ctrlKey: true, altKey: true }),
    k("F5", { ctrlKey: true }), k("a", { metaKey: true }), k("Tab", { ctrlKey: true }), k(",", { ctrlKey: true }),
  ])("null für %j", (event) => expect(eventToSpec(event)).toBeNull());
});
