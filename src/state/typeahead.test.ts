import { describe, expect, it } from "vitest";
import type { Entry } from "../ipc/types";
import { createTypeahead, isTypeaheadKey, resetTypeahead, typeahead } from "./typeahead";

const e = (name: string): Entry => ({
  name,
  kind: "file",
  size: 0,
  modified: 0,
  mode: null,
  hidden: false,
  linkTarget: null,
  brokenLink: false,
  error: null,
});
const entries = ["..", "alpha", "Anton", "beta", "bild", "bilder", "gamma"].map(e);

describe("typeahead", () => {
  it("springt mit einem Buchstaben zum ersten passenden Eintrag ab dem Fokus", () => {
    const r = typeahead(createTypeahead(), "b", 0, entries, 0);
    expect(r.focus).toBe(3);
    expect(r.matched).toBe(true);
  });
  it("sammelt mehrere Buchstaben als Präfix, ohne Groß-/Kleinschreibung", () => {
    let r = typeahead(createTypeahead(), "B", 0, entries, 0);
    r = typeahead(r.state, "i", 100, entries, r.focus);
    r = typeahead(r.state, "l", 200, entries, r.focus);
    r = typeahead(r.state, "d", 300, entries, r.focus);
    r = typeahead(r.state, "e", 400, entries, r.focus);
    expect(entries[r.focus]!.name).toBe("bilder");
    expect(r.state.buffer).toBe("Bilde");
  });
  it("setzt nach abgelaufenem Zeitfenster zurück", () => {
    let r = typeahead(createTypeahead(), "b", 0, entries, 0);
    r = typeahead(r.state, "e", 100, entries, r.focus);
    expect(entries[r.focus]!.name).toBe("beta");
    r = typeahead(r.state, "g", 2000, entries, r.focus);
    expect(r.state.buffer).toBe("g");
    expect(entries[r.focus]!.name).toBe("gamma");
  });
  it("lässt den Fokus bei keinem Treffer unverändert", () => {
    const r = typeahead(createTypeahead(), "x", 0, entries, 4);
    expect(r.focus).toBe(4);
    expect(r.matched).toBe(false);
  });
  it("sucht ringsum und überspringt ..", () => {
    const r = typeahead(createTypeahead(), "a", 0, entries, 5);
    expect(entries[r.focus]!.name).toBe("alpha");
    expect(typeahead(createTypeahead(), ".", 0, entries, 0).matched).toBe(false);
  });
  it("wiederholter Buchstabe springt reihum weiter", () => {
    let r = typeahead(createTypeahead(), "a", 0, entries, 0);
    expect(entries[r.focus]!.name).toBe("alpha");
    r = typeahead(r.state, "a", 100, entries, r.focus);
    expect(entries[r.focus]!.name).toBe("Anton");
    r = typeahead(r.state, "a", 200, entries, r.focus);
    expect(entries[r.focus]!.name).toBe("alpha");
  });
  it("resetTypeahead leert den Puffer (Esc)", () => {
    const r = typeahead(createTypeahead(), "b", 0, entries, 0);
    expect(resetTypeahead().buffer).toBe("");
    const again = typeahead(resetTypeahead(), "g", 10, entries, r.focus);
    expect(entries[again.focus]!.name).toBe("gamma");
  });
  it("funktioniert mit leerer Liste", () => {
    expect(typeahead(createTypeahead(), "a", 0, [], 0)).toMatchObject({ focus: 0, matched: false });
  });
  it("erkennt druckbare Tasten ohne Strg/Alt", () => {
    expect(isTypeaheadKey({ key: "a" })).toBe(true);
    expect(isTypeaheadKey({ key: "ä" })).toBe(true);
    expect(isTypeaheadKey({ key: "a", ctrlKey: true })).toBe(false);
    expect(isTypeaheadKey({ key: "Enter" })).toBe(false);
    expect(isTypeaheadKey({ key: " " })).toBe(false);
  });
});
