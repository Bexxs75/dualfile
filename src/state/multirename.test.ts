import { describe, expect, test } from "vitest";
import { createMockApi } from "../ipc/mock";
import type { EntryKind, MediaTags } from "../ipc/types";
import {
  applyCase,
  defaultRules,
  emptyTags,
  deletePreset,
  needsRegex,
  parseRules,
  planRename,
  regexTargets,
  replaceText,
  sanitizePresets,
  savePreset,
  splitName,
  type MultiRenameInput,
  type MultiRenameRules,
  type RenameSource,
} from "./multirename";

const MODIFIED = new Date(2026, 10, 5, 8, 7, 9).getTime(); // lokale Zeit: 05.11.2026 08:07:09

const file = (name: string, modified: number | null = MODIFIED): RenameSource => ({ name, kind: "file", modified, tags: emptyTags });
const dir = (name: string): RenameSource => ({ name, kind: "dir" as EntryKind, modified: MODIFIED, tags: emptyTags });

function input(sources: RenameSource[], rules: Partial<MultiRenameRules> = {}, extra: Partial<MultiRenameInput> = {}): MultiRenameInput {
  return {
    sources,
    rules: { ...defaultRules, ...rules },
    parentName: "Urlaub",
    existingNames: sources.map((s) => s.name),
    ...extra,
  };
}
const names = (i: MultiRenameInput): string[] => planRename(i).rows.map((row) => row.newName);

describe("Namen zerlegen", () => {
  test("Datei: Endung ab dem letzten Punkt", () => {
    expect(splitName("foto.tar.gz", "file")).toEqual({ base: "foto.tar", ext: "gz" });
  });
  test("Ordner haben nie eine Endung", () => {
    expect(splitName("Backup.2026", "dir")).toEqual({ base: "Backup.2026", ext: "" });
  });
  test("versteckte Dateien (.bashrc), Dateien ohne Punkt und mit Punkt am Ende haben keine Endung", () => {
    expect(splitName(".bashrc", "file")).toEqual({ base: ".bashrc", ext: "" });
    expect(splitName("Makefile", "file")).toEqual({ base: "Makefile", ext: "" });
    expect(splitName("name.", "file")).toEqual({ base: "name.", ext: "" });
  });
});

describe("Platzhalter der Maske", () => {
  test("[N] und [E] getrennt für Name und Endung", () => {
    expect(names(input([file("bild.jpg")], { nameMask: "neu_[N]", extMask: "[E]" }))).toEqual(["neu_bild.jpg"]);
    expect(names(input([file("bild.jpg")], { nameMask: "[N]", extMask: "png" }))).toEqual(["bild.png"]);
  });

  test("leere Endungsmaske entfernt die Endung samt Punkt", () => {
    expect(names(input([file("bild.jpg")], { extMask: "" }))).toEqual(["bild"]);
  });

  test("Zähler [C]: Start, Schrittweite, Stellen mit führenden Nullen", () => {
    const sources = [file("a.txt"), file("b.txt"), file("c.txt")];
    expect(names(input(sources, { nameMask: "x[C]", counterStart: 5, counterStep: 10, counterDigits: 3 }))).toEqual(["x005.txt", "x015.txt", "x025.txt"]);
    expect(names(input(sources, { nameMask: "x[C]" }))).toEqual(["x1.txt", "x2.txt", "x3.txt"]);
  });

  test("Zähler zählt in der Reihenfolge der übergebenen Liste", () => {
    expect(names(input([file("z.txt"), file("a.txt")], { nameMask: "[C]" }))).toEqual(["1.txt", "2.txt"]);
  });

  test("Zähler mit Schrittweite 0 ergibt doppelte Namen, negativer Wert ist ein Fehler", () => {
    const sources = [file("a.txt"), file("b.txt")];
    const same = planRename(input(sources, { nameMask: "n[C]", counterStep: 0 }));
    expect(same.rows.map((r) => r.errorKey)).toEqual(["multirename.error.duplicate", "multirename.error.duplicate"]);
    const negative = planRename(input(sources, { nameMask: "n[C]", counterStart: 0, counterStep: -1 }));
    expect(negative.rows[1]?.errorKey).toBe("multirename.error.counterRange");
  });

  test("Datum aus der Änderungszeit (lokale Zeit, zweistellig außer dem Jahr)", () => {
    expect(names(input([file("a.txt")], { nameMask: "[Y]-[M]-[D]_[h][m][s]" }))).toEqual(["2026-11-05_080709.txt"]);
  });

  test("Datumsplatzhalter ohne Änderungszeit sind ein Fehler dieses Eintrags", () => {
    const plan = planRename(input([file("a.txt", null), file("b.txt")], { nameMask: "[Y]" }));
    expect(plan.rows[0]).toMatchObject({ status: "error", errorKey: "multirename.error.noDate", errorParams: { token: "Y" } });
    expect(plan.rows[1]?.status).toBe("ok");
  });

  test("[P] ist der Name des übergeordneten Ordners", () => {
    expect(names(input([file("a.txt")], { nameMask: "[P]_[N]" }))).toEqual(["Urlaub_a.txt"]);
  });

  test("[=Text] ist wörtlich, auch mit Platzhalter-Buchstaben", () => {
    expect(names(input([file("a.txt")], { nameMask: "[=N-C]x" }))).toEqual(["N-Cx.txt"]);
  });

  test("[[ und ]] ergeben wörtliche Klammern", () => {
    expect(names(input([file("a.txt")], { nameMask: "[[[N]]]" }))).toEqual(["[a].txt"]);
  });

  test("Zeichenbereiche [N1-3], [N2-], [N-3] und einzelnes Zeichen [N2]", () => {
    const one = [file("abcdef.txt")];
    expect(names(input(one, { nameMask: "[N1-3]" }))).toEqual(["abc.txt"]);
    expect(names(input(one, { nameMask: "[N2-]" }))).toEqual(["bcdef.txt"]);
    expect(names(input(one, { nameMask: "[N-3]" }))).toEqual(["abc.txt"]);
    expect(names(input(one, { nameMask: "[N2]" }))).toEqual(["b.txt"]);
    expect(names(input(one, { nameMask: "[N4-99]" }))).toEqual(["def.txt"]);
    expect(names(input(one, { nameMask: "[N9-12]x" }))).toEqual(["x.txt"]);
  });

  test("Zeichenbereiche zählen Zeichen, nicht UTF-16-Einheiten", () => {
    expect(names(input([file("😀äb.txt")], { nameMask: "[N1-2]" }))).toEqual(["😀ä.txt"]);
  });

  test("ungültige Bereiche (0, rückwärts) sind ein Fehler", () => {
    for (const mask of ["[N0-2]", "[N3-1]"]) {
      expect(planRename(input([file("abcdef.txt")], { nameMask: mask })).rows[0]).toMatchObject({ status: "error", errorKey: "multirename.error.range" });
    }
  });

  test("unbekannter Platzhalter ist ein Fehler in der Vorschau, nie stilles Ignorieren", () => {
    const plan = planRename(input([file("a.txt"), file("b.txt")], { nameMask: "[X]" }));
    expect(plan.rows.map((r) => [r.status, r.errorKey, r.errorParams])).toEqual([
      ["error", "multirename.error.placeholder", { token: "X" }],
      ["error", "multirename.error.placeholder", { token: "X" }],
    ]);
    expect(plan.canApply).toBe(false);
    expect(plan.items).toEqual([]);
    for (const mask of ["[Name]", "[]", "[c]", "[CC]"]) {
      expect(planRename(input([file("a.txt")], { nameMask: mask })).rows[0]?.errorKey, mask).toBe("multirename.error.placeholder");
    }
  });

  test("nicht geschlossene und nicht geöffnete Klammern sind Fehler", () => {
    expect(planRename(input([file("a.txt")], { nameMask: "x[N" })).rows[0]?.errorKey).toBe("multirename.error.unclosed");
    expect(planRename(input([file("a.txt")], { nameMask: "x]" })).rows[0]?.errorKey).toBe("multirename.error.unopened");
  });

  test("Fehler in der Endungsmaske betreffen Dateien, Ordner ignorieren die Endungsmaske", () => {
    const plan = planRename(input([file("a.txt"), dir("Ordner")], { extMask: "[Q]" }));
    expect(plan.rows[0]?.errorKey).toBe("multirename.error.placeholder");
    expect(plan.rows[1]?.status).toBe("unchanged");
  });

  test("Ordner: ganzer Name ist [N], [E] bleibt leer, Punkte im Namen werden nicht getrennt", () => {
    expect(names(input([dir("Backup.2026")], { nameMask: "[N]_alt", extMask: "[E]" }))).toEqual(["Backup.2026_alt"]);
  });

  test(".bashrc und Dateien ohne Endung: [E] ist leer, kein Punkt am Ende", () => {
    expect(names(input([file(".bashrc"), file("Makefile")], { nameMask: "[N]2" }))).toEqual([".bashrc2", "Makefile2"]);
    expect(names(input([file("Makefile")], { nameMask: "[N]", extMask: "txt" }))).toEqual(["Makefile.txt"]);
  });
});

describe("Suchen und Ersetzen (Text)", () => {
  test("alle Treffer oder nur der erste", () => {
    expect(replaceText("a-b-c", "-", "_", false, true)).toBe("a_b_c");
    expect(replaceText("a-b-c", "-", "_", false, false)).toBe("a_b-c");
  });
  test("mit und ohne Beachtung der Groß-/Kleinschreibung", () => {
    expect(replaceText("Foto FOTO foto", "foto", "x", false, true)).toBe("Foto FOTO x");
    expect(replaceText("Foto FOTO foto", "foto", "x", true, true)).toBe("x x x");
  });
  test("Sonderzeichen gelten wörtlich (kein Regex)", () => {
    expect(replaceText("a.b(1)", ".", "!", false, true)).toBe("a!b(1)");
    expect(replaceText("a(1)", "(1)", "$1", false, true)).toBe("a$1");
  });
  test("leeres Suchmuster ändert nichts; Ersetzen durch nichts löscht", () => {
    expect(replaceText("abc", "", "x", false, true)).toBe("abc");
    expect(replaceText("abc", "b", "", false, true)).toBe("ac");
  });
  test("in der Vorschau wirkt Text-Ersetzen auf den Namen nach der Maske", () => {
    expect(names(input([file("urlaub_a.txt")], { nameMask: "[N]_x", search: "_", replace: "-" }))).toEqual(["urlaub-a-x.txt"]);
  });
});

describe("Regex über den Kern (Mock)", () => {
  const api = createMockApi();
  async function viaMock(i: MultiRenameInput) {
    const results = await api.regexReplace(regexTargets(i), i.rules.search, i.rules.replace, i.rules.ignoreCase, i.rules.all);
    return planRename(i, { kind: "results", results });
  }

  test("braucht nur im Regex-Modus mit Muster eine Antwort des Kerns", () => {
    expect(needsRegex({ ...defaultRules, searchMode: "regex", search: "a" })).toBe(true);
    expect(needsRegex({ ...defaultRules, searchMode: "regex", search: "" })).toBe(false);
    expect(needsRegex({ ...defaultRules, searchMode: "text", search: "a" })).toBe(false);
  });

  test("Gruppen $1 im Ersatz", async () => {
    const plan = await viaMock(input([file("IMG_0412.jpg"), file("IMG_0413.jpg")], { searchMode: "regex", search: "IMG_(\\d+)", replace: "Foto-$1" }));
    expect(plan.rows.map((r) => r.newName)).toEqual(["Foto-0412.jpg", "Foto-0413.jpg"]);
    expect(plan.canApply).toBe(true);
  });

  test("ohne Antwort des Kerns bleibt die Ersetzung unberücksichtigt", () => {
    const plan = planRename(input([file("IMG_1.jpg")], { searchMode: "regex", search: "IMG", replace: "X" }));
    expect(plan.rows[0]?.newName).toBe("IMG_1.jpg");
  });

  test("regexTargets liefert nur Namen ohne Maskenfehler, in Listenreihenfolge", () => {
    const i = input([file("a.txt"), file("b.txt", null), file("c.txt")], { nameMask: "[Y]-[N]", searchMode: "regex", search: "x" });
    expect(regexTargets(i)).toEqual(["2026-a.txt", "2026-c.txt"]);
  });

  test("Fehlerantwort des Kerns (ungültiger Ausdruck) macht alle Einträge zu Fehlern und sperrt", async () => {
    const i = input([file("a.txt"), file("b.txt")], { searchMode: "regex", search: "(", replace: "" });
    let message = "";
    try {
      await api.regexReplace(regexTargets(i), i.rules.search, i.rules.replace, false, true);
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).not.toBe("");
    const plan = planRename(i, { kind: "failed", message });
    expect(plan.rulesError?.key).toBe("multirename.error.regex");
    expect(plan.rows.every((r) => r.status === "error" && r.errorParams?.message === message)).toBe(true);
    expect(plan.canApply).toBe(false);
  });

  test("Fehler eines einzelnen Namens betrifft nur diese Zeile", () => {
    const plan = planRename(input([file("a.txt"), file("b.txt")], { searchMode: "regex", search: "a", replace: "x" }), {
      kind: "results",
      results: [{ kind: "error", message: "zu lang" }, { kind: "ok", name: "b.txt" }],
    });
    expect(plan.rows[0]).toMatchObject({ status: "error", errorKey: "multirename.error.regex", errorParams: { message: "zu lang" } });
    expect(plan.rows[1]?.status).toBe("unchanged");
  });
});

describe("Groß/Klein", () => {
  test.each([
    ["keep", "Mein foto-Album.JPG", "Mein foto-Album.JPG"],
    ["upper", "Mein foto.jpg", "MEIN FOTO.JPG"],
    ["lower", "Mein FOTO.JPG", "mein foto.jpg"],
    ["words", "mein foto-album_2026.jpg", "Mein Foto-Album_2026.Jpg"],
    ["first", "mEIN fOTO.JPG", "Mein foto.jpg"],
  ] as const)("Modus %s", (mode, before, after) => {
    expect(applyCase(before, mode)).toBe(after);
  });
  test("Wortanfänge zählen auch Umlaute als Buchstaben, Ziffern beenden kein Wort", () => {
    expect(applyCase("äpfel und 3rd BIRNEN", "words")).toBe("Äpfel Und 3rd Birnen");
  });
  test("läuft nach Maske und Ersetzen", () => {
    expect(names(input([file("a_b.txt")], { search: "_", replace: " ", caseMode: "words" }))).toEqual(["A B.Txt"]);
  });
});

describe("Prüfung der Vorschau", () => {
  test("Ergebnis gleich Quellname ist „unverändert“, nicht ein Fehler", () => {
    const plan = planRename(input([file("a.txt")]));
    expect(plan.rows[0]).toMatchObject({ oldName: "a.txt", newName: "a.txt", status: "unchanged" });
    expect(plan.counts).toEqual({ ok: 0, unchanged: 1, error: 0 });
    expect(plan.canApply).toBe(false); // nichts zu tun
  });

  test("ungültiger Name: leer, Schrägstrich, zu lang", () => {
    expect(planRename(input([file("a.txt")], { nameMask: "", extMask: "" })).rows[0]?.errorKey).toBe("error.name.empty");
    expect(planRename(input([file("a.txt")], { nameMask: "x/[N]" })).rows[0]?.errorKey).toBe("error.name.slash");
    expect(planRename(input([file("a.txt")], { nameMask: "x".repeat(260) })).rows[0]?.errorKey).toBe("error.name.tooLong");
    expect(planRename(input([file("a.txt")], { nameMask: "..", extMask: "" })).rows[0]?.errorKey).toBe("error.name.dot");
  });

  test("doppelter neuer Name in der Auswahl: alle Beteiligten sind Fehler", () => {
    const plan = planRename(input([file("a.txt"), file("b.txt"), file("c.txt")], { nameMask: "gleich" }));
    expect(plan.rows.map((r) => r.status)).toEqual(["error", "error", "error"]);
    expect(plan.rows.every((r) => r.errorKey === "multirename.error.duplicate")).toBe(true);
  });

  test("Kollision mit einem NICHT umbenannten Eintrag des Ordners ist ein Fehler", () => {
    const plan = planRename(input([file("a.txt")], { nameMask: "fremd" }, { existingNames: ["a.txt", "fremd.txt"] }));
    expect(plan.rows[0]).toMatchObject({ status: "error", errorKey: "error.name.exists" });
  });

  test("Kollision mit einem unverändert bleibenden Eintrag der Auswahl ist ein Fehler", () => {
    const plan = planRename(input([file("a.txt"), file("b.txt")], { nameMask: "[N]", search: "b", replace: "a", all: true }));
    // a.txt bleibt a.txt (unverändert), b.txt würde a.txt heißen
    expect(plan.rows.map((r) => r.status)).toEqual(["unchanged", "error"]);
    expect(plan.rows[1]?.errorKey).toBe("error.name.exists");
  });

  test("Tausch a↔b ist erlaubt", () => {
    // echter Tausch über Zähler rückwärts: erstes Element -> Namen des zweiten und umgekehrt
    const reverse = planRename(
      input([file("1.txt"), file("2.txt")], { nameMask: "[C]", counterStart: 2, counterStep: -1 }, { existingNames: ["1.txt", "2.txt"] }),
    );
    expect(reverse.rows.map((r) => [r.oldName, r.newName, r.status])).toEqual([
      ["1.txt", "2.txt", "ok"],
      ["2.txt", "1.txt", "ok"],
    ]);
    expect(reverse.canApply).toBe(true);
  });

  test("Kette a→b, b→c ist erlaubt", () => {
    const plan = planRename(
      input([file("1.txt"), file("2.txt")], { nameMask: "[C]", counterStart: 2 }, { existingNames: ["1.txt", "2.txt"] }),
    );
    expect(plan.rows.map((r) => [r.newName, r.status])).toEqual([["2.txt", "ok"], ["3.txt", "ok"]]);
    expect(plan.canApply).toBe(true);
  });

  test("items enthalten nur geänderte Einträge; bei Fehlern ist die Liste leer", () => {
    const ok = planRename(input([file("a.txt"), file("b.txt")], { nameMask: "[N]", search: "a", replace: "z" }));
    expect(ok.items).toEqual([{ oldName: "a.txt", newName: "z.txt" }]);
    expect(ok.counts).toEqual({ ok: 1, unchanged: 1, error: 0 });
    const bad = planRename(input([file("a.txt"), file("b.txt")], { nameMask: "[X]" }));
    expect(bad.items).toEqual([]);
  });

  test("ungültige Zählerwerte sind ein Fehler der Regeln und sperren alles", () => {
    for (const patch of [{ counterStart: Number.NaN }, { counterDigits: 0 }, { counterDigits: 12 }, { counterStep: 1.5 }, { counterStart: -1 }]) {
      const plan = planRename(input([file("a.txt")], patch));
      expect(plan.rulesError?.key, JSON.stringify(patch)).toBe("multirename.error.counterInvalid");
      expect(plan.canApply).toBe(false);
    }
  });
});

describe("Voreinstellungen", () => {
  const rules: MultiRenameRules = { ...defaultRules, nameMask: "Foto_[C]", counterDigits: 3, caseMode: "lower" };

  test("speichern ersetzt gleichnamige, löschen entfernt, leere Namen werden ignoriert", () => {
    let presets = savePreset([], "  Fotos ", rules);
    expect(presets.map((p) => p.name)).toEqual(["Fotos"]);
    presets = savePreset(presets, "Fotos", { ...rules, nameMask: "x" });
    expect(presets).toHaveLength(1);
    expect(presets[0]?.rules.nameMask).toBe("x");
    expect(savePreset(presets, "   ", rules)).toEqual(presets);
    expect(deletePreset(presets, "Fotos")).toEqual([]);
  });

  test("Roundtrip über JSON liefert dieselben Regeln", () => {
    const stored = JSON.parse(JSON.stringify(savePreset([], "A", rules))) as unknown;
    expect(sanitizePresets(stored)).toEqual([{ name: "A", rules }]);
  });

  test("beim Laden werden Typen, Längen und Bereiche geprüft", () => {
    expect(parseRules({ ...rules, nameMask: 5 })).toBeNull();
    expect(parseRules({ ...rules, caseMode: "shout" })).toBeNull();
    expect(parseRules({ ...rules, searchMode: "glob" })).toBeNull();
    expect(parseRules({ ...rules, nameMask: "x".repeat(2000) })).toBeNull();
    expect(parseRules({ ...rules, counterDigits: 99 })).toBeNull();
    expect(parseRules({ ...rules, counterStart: "1" })).toBeNull();
    expect(parseRules(null)).toBeNull();
    expect(parseRules(rules)).toEqual(rules);
    const raw = [{ name: "gut", rules }, { name: "", rules }, { name: "schlecht", rules: { ...rules, all: "ja" } }, { name: "gut", rules }, 7, null];
    expect(sanitizePresets(raw).map((p) => p.name)).toEqual(["gut"]);
    expect(sanitizePresets("kein Array")).toEqual([]);
  });
});

describe("Mehrfach-Umbenennen: EXIF-/ID3-Platzhalter", () => {
  const tagged = (name: string, tags: Partial<MediaTags>): RenameSource => ({ ...file(name), tags: { ...emptyTags, ...tags } });
  test("EXIF-Datum und Kamera", () => {
    const s = tagged("a.jpg", { taken: "2021:07:15 14:32:08", camera: "Canon EOS R5" });
    expect(names(input([s], { nameMask: "[XY]-[XM]-[XD]_[Xh][Xm][Xs] [Xc]" }))).toEqual(["2021-07-15_143208 Canon EOS R5.jpg"]);
  });
  test("fehlendes EXIF-Datum ist ein Fehler, leere Kamera nicht", () => {
    const s = tagged("a.jpg", {});
    expect(planRename(input([s], { nameMask: "[XY]" })).rows[0]).toMatchObject({ status: "error", errorKey: "multirename.error.noExifDate", errorParams: { token: "XY" } });
    expect(planRename(input([s], { nameMask: "x[Xc]" })).rows[0]).toMatchObject({ newName: "x.jpg" });
  });
  test("unbekannter Buchstabe ist ein Platzhalterfehler", () => {
    const s = tagged("a.jpg", { taken: "2021:07:15 14:32:08" });
    expect(planRename(input([s], { nameMask: "[Xz]" })).rows[0]).toMatchObject({ errorKey: "multirename.error.placeholder" });
    expect(planRename(input([s], { nameMask: "[Tz]" })).rows[0]).toMatchObject({ errorKey: "multirename.error.placeholder" });
  });
  test("ID3-Tags, pfadunsichere Zeichen werden entfernt", () => {
    const s = tagged("a.mp3", { artist: "AC/DC", title: "Back\\In\nBlack", album: "Alb", track: 3 });
    expect(names(input([s], { nameMask: "[Ta] - [Tt] - [Tb] - [Tn]" }))).toEqual(["ACDC - BackInBlack - Alb - 3.mp3"]);
  });
  test("solange die Tags laden, sperren Tag-Masken, andere nicht", () => {
    const s = tagged("a.mp3", { artist: "X" });
    const pending = planRename({ ...input([s], { nameMask: "[Ta]" }), tagsPending: true });
    expect(pending.canApply).toBe(false);
    expect(pending.rulesError?.key).toBe("multirename.error.tagsLoading");
    // Eine Maske ohne Tag-Platzhalter bleibt auch während des Ladens anwendbar.
    expect(planRename({ ...input([s], { nameMask: "neu" }), tagsPending: true }).canApply).toBe(true);
  });
});
