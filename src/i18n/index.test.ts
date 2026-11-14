import { describe, expect, test } from "vitest";
import de from "./de";
import en from "./en";
import {
  createTranslator,
  errorKey,
  formatBytes,
  formatDate,
  resolveLanguage,
  t,
  type TranslationKey,
} from "./index";

const placeholders = (text: string): string[] => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1] ?? "").sort();

describe("Wörterbücher", () => {
  test("Deutsch und Englisch haben genau dieselben Schlüssel", () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(de).sort());
  });

  test("keine Übersetzung ist leer", () => {
    for (const [key, value] of Object.entries(de)) expect(value.trim(), `de ${key}`).not.toBe("");
    for (const [key, value] of Object.entries(en)) expect(value.trim(), `en ${key}`).not.toBe("");
  });

  test("beide Sprachen verwenden in jedem Text dieselben Platzhalter", () => {
    const english: Record<string, string> = en;
    for (const [key, value] of Object.entries(de)) {
      expect(placeholders(english[key] ?? ""), key).toEqual(placeholders(value));
    }
  });

  test("zu jeder Mehrzahlform gibt es .one und .other", () => {
    for (const dictionary of [de, en] as Record<string, string>[]) {
      const keys = Object.keys(dictionary);
      for (const key of keys.filter((k) => k.endsWith(".one"))) {
        expect(keys).toContain(`${key.slice(0, -4)}.other`);
      }
      for (const key of keys.filter((k) => k.endsWith(".other"))) {
        expect(keys).toContain(`${key.slice(0, -6)}.one`);
      }
    }
  });

  test("der Löschdialog sagt in beiden Sprachen ausdrücklich, dass es nicht rückgängig zu machen ist", () => {
    expect(t("dialog.delete.warning", {}, "de")).toMatch(/nicht rückgängig/);
    expect(t("dialog.delete.warning", {}, "en")).toMatch(/cannot be undone/);
    expect(t("dialog.delete.partialWarning", {}, "de")).toMatch(/endgültig entfernt/);
    expect(t("dialog.delete.partialWarning", {}, "en")).toMatch(/permanently removed/);
  });

  test("alle Oberflächenbereiche des Plans sind vertreten", () => {
    const prefixes = [
      "menu.", "context.", "fkey.", "dialog.conflict.", "dialog.trash.", "dialog.delete.", "dialog.rename.",
      "dialog.newFolder.", "dialog.properties.", "status.", "settings.", "bookmarks.", "tip.", "error.", "jobs.",
      "toast.", "notice.", "column.", "multirename.", "archive.", "pack.", "dialog.pack.", "dialog.archiveError.", "dialog.extract.", "compare.", "sync.", "diff.",
    ];
    for (const prefix of prefixes) {
      expect(Object.keys(de).some((key) => key.startsWith(prefix)), prefix).toBe(true);
    }
    for (const key of ["fkey.f3", "fkey.f4", "fkey.f5", "fkey.f6", "fkey.f7", "fkey.f8"]) expect(de).toHaveProperty(key);
  });
});

describe("Vergleichen (Phase 5c)", () => {
  test("Überschreiben und Papierkorb heißen in beiden Sprachen ausdrücklich so", () => {
    expect(t("sync.kind.replace", {}, "de")).toBe("ÜBERSCHREIBEN");
    expect(t("sync.kind.trash", {}, "de")).toBe("IN DEN PAPIERKORB");
    expect(t("sync.kind.replace", {}, "en")).toBe("OVERWRITE");
    expect(t("sync.kind.trash", {}, "en")).toBe("MOVE TO TRASH");
  });
  test("jeder Vergleichszustand hat in beiden Sprachen einen Text", () => {
    for (const state of ["same", "onlyLeft", "onlyRight", "leftNewer", "rightNewer", "different", "typeMismatch", "unknown"]) {
      expect(de).toHaveProperty(`compare.state.${state}`);
      expect(en).toHaveProperty(`compare.state.${state}`);
    }
  });
  test("Bestätigungsknopf mit Zähler", () => {
    expect(t("sync.confirm", { parts: "3 überschreiben, 2 in den Papierkorb" }, "de")).toBe("Ausführen: 3 überschreiben, 2 in den Papierkorb");
    expect(t("sync.confirm", { parts: "3 to overwrite" }, "en")).toBe("Run: 3 to overwrite");
  });
});

describe("t", () => {
  test("setzt Platzhalter ein", () => {
    expect(t("error.notFound", { path: "/tmp/x" }, "en")).toBe("Not found: /tmp/x");
    expect(t("status.selected", { selected: 2, total: 9 }, "de")).toBe("2 von 9 markiert");
    expect(t("status.selected", { selected: 2, total: 9 }, "en")).toBe("2 of 9 selected");
  });

  test("Standardsprache ist Deutsch", () => {
    expect(t("action.cancel")).toBe("Abbrechen");
    expect(t("action.cancel", {}, "en")).toBe("Cancel");
  });

  test("ein Platzhalter kommt auch mehrfach vor und Werte werden nicht erneut ausgewertet", () => {
    expect(t("jobs.summary.delete", { deleted: "{failed}", failed: 3 }, "de")).toBe("{failed} gelöscht, 3 nicht gelöscht");
  });

  test("ein fehlender Platzhalter bleibt sichtbar stehen", () => {
    expect(t("error.notFound", {}, "de")).toBe("Nicht gefunden: {path}");
  });

  test("einfache Mehrzahl: Einzahl bei 1, sonst Mehrzahl (auch bei 0)", () => {
    expect(t("status.items", { count: 1 }, "de")).toBe("1 Eintrag");
    expect(t("status.items", { count: 0 }, "de")).toBe("0 Einträge");
    expect(t("status.items", { count: 12 }, "de")).toBe("12 Einträge");
    expect(t("status.items", { count: 1 }, "en")).toBe("1 item");
    expect(t("status.items", { count: 2 }, "en")).toBe("2 items");
  });

  test("Mehrzahl mit weiteren Platzhaltern", () => {
    expect(t("dialog.trash.body", { count: 1, name: "a.txt" }, "de")).toBe("„a.txt“ wird in den Papierkorb gelegt.");
    expect(t("dialog.trash.body", { count: 3, name: "a.txt" }, "de")).toBe("3 Elemente werden in den Papierkorb gelegt.");
    expect(t("dialog.delete.dirs", { count: 1 }, "de")).toBe("1 Ordner");
    expect(t("dialog.delete.dirs", { count: 4 }, "de")).toBe("4 Ordner");
    expect(t("dialog.delete.files", { count: 1 }, "en")).toBe("1 file");
  });

  test("Platzbelegung, Duplikate und Beenden-Rückfrage: Einzahl bei 1", () => {
    const one: [TranslationKey, Record<string, string | number>][] = [
      ["space.progressCount", { size: "1 B" }], ["space.issues", {}], ["space.excess", { size: "1 B" }],
      ["space.summary", { size: "1 B", seconds: 1 }], ["space.files", {}], ["dupes.summary", { size: "1 B" }],
      ["dupes.group", { size: "1 B", reclaimable: "1 B" }], ["dupes.skipped", {}], ["closeConfirm.message", {}],
    ];
    for (const language of ["de", "en"] as const) {
      for (const [key, params] of one) {
        const single = t(key, { ...params, count: 1 }, language);
        const many = t(key, { ...params, count: 2 }, language);
        expect(single, `${language} ${key}`).not.toBe(many);
        expect(single).not.toMatch(/\b1 (files|folders|entries|groups|jobs|Dateien|Einträge|Gruppen|Aufträge)\b/);
      }
    }
    expect(t("space.files", { count: 1 }, "en")).toBe("1 file");
    expect(t("space.files", { count: 1 }, "de")).toBe("1 Datei");
    expect(t("space.files", { count: 3 }, "de")).toBe("3 Dateien");
    expect(t("dupes.skipped", { count: 1 }, "en")).toBe("1 file skipped");
    expect(t("space.excess", { count: 1, size: "5 B" }, "de")).toBe("1 Datei, 5 B überzählig");
    expect(t("closeConfirm.message", { count: 1 }, "en")).toBe("There is still 1 active job. Quitting cancels it.");
  });

  test("Fehlerzusammenfassung „x von y nicht kopiert“", () => {
    expect(t("jobs.summary.copy", { failed: 3, total: 120 }, "de")).toBe("3 von 120 nicht kopiert");
    expect(t("jobs.summary.copy", { failed: 3, total: 120 }, "en")).toBe("3 of 120 not copied");
  });

  test("unbekannter Schlüssel liefert den Schlüssel selbst", () => {
    expect(t("gibt.es.nicht" as TranslationKey)).toBe("gibt.es.nicht");
  });

  test("createTranslator bindet die Sprache", () => {
    const tr = createTranslator("en");
    expect(tr("menu.file")).toBe("File");
    expect(tr("status.items", { count: 5 })).toBe("5 items");
  });
});

describe("resolveLanguage", () => {
  test("feste Einstellung gewinnt gegen die Systemsprache", () => {
    expect(resolveLanguage("en", "de-DE")).toBe("en");
    expect(resolveLanguage("de", "en-US")).toBe("de");
  });

  test("auto: deutsche Systemsprachen (auch regional, auch Großschreibung) ergeben Deutsch", () => {
    expect(resolveLanguage("auto", "de")).toBe("de");
    expect(resolveLanguage("auto", "de-CH")).toBe("de");
    expect(resolveLanguage("auto", "DE-at")).toBe("de");
  });

  test("auto: alles andere und fehlende Angaben ergeben Englisch", () => {
    expect(resolveLanguage("auto", "fr-FR")).toBe("en");
    expect(resolveLanguage("auto", "en-GB")).toBe("en");
    expect(resolveLanguage("auto", "")).toBe("en");
    expect(resolveLanguage("auto", undefined)).toBe("en");
  });
});

describe("Mehrfach-Umbenennen: Texte", () => {
  test("jeder Platzhalter des Einfügemenüs und jeder Fehlerschlüssel der Berechnung ist in beiden Sprachen übersetzt", async () => {
    const { PLACEHOLDERS } = await import("../state/multirename");
    for (const placeholder of PLACEHOLDERS) {
      for (const language of ["de", "en"] as const) {
        expect(t(placeholder.label, {}, language), `${language} ${placeholder.label}`).not.toBe(placeholder.label);
      }
    }
    const keys = Object.keys(de).filter((key) => key.startsWith("multirename.error."));
    expect(keys.length).toBeGreaterThanOrEqual(9);
    for (const key of keys) expect(Object.keys(en)).toContain(key);
  });
});

describe("errorKey", () => {
  test("Fehlerarten des Mocks haben jeweils einen eigenen Text, Unbekanntes einen allgemeinen", () => {
    const kinds = ["notFound", "invalidName", "exists", "permission", "notDirectory", "invalidTarget", "invalidToken", "planChanged", "cancelled", "readOnly", "archive", "io", "disconnected", "auth", "timeout", "trust", "algorithmNotAllowed", "remote", "sizeMismatch", "pinMismatch", "tooManyConnections", "secretRemovalFailed", "pluginLocked", "pluginSandboxUnavailable", "pluginTimeout", "pluginProtocol", "pluginDisabled", "pluginCrashed", "pluginTooLarge", "pluginNotInstalled", "pluginUnsafe", "pluginBusy", "pluginFailed"];
    const keys = kinds.map(errorKey);
    expect(new Set(keys).size).toBe(kinds.length);
    for (const key of keys) expect(t(key, { path: "p", name: "n", reason: "r" }, "de")).not.toBe(key);
    expect(errorKey("irgendwas")).toBe("error.unknown");
  });
});

describe("Formate", () => {
  test("formatBytes: Einheiten und Dezimalkomma", () => {
    expect(formatBytes(0, "de")).toBe("0 B");
    expect(formatBytes(1023, "de")).toBe("1023 B");
    expect(formatBytes(1536, "de")).toBe("1,5 KB");
    expect(formatBytes(1536, "en")).toBe("1.5 KB");
    expect(formatBytes(3_400_000, "de")).toBe("3,2 MB");
    expect(formatBytes(20_000_000_000, "en")).toBe("18.6 GB");
  });

  test("formatDate: deutsch und englisch, unbekannt ist leer", () => {
    const stamp = Date.UTC(2026, 10, 12, 8, 5);
    expect(formatDate(stamp, "de", "UTC")).toBe("12.11.2026 08:05");
    expect(formatDate(stamp, "en", "UTC")).toBe("2026-11-12 08:05");
    expect(formatDate(null, "de")).toBe("");
  });
});

test("similar-image counts have German and English singular and plural forms",()=>{
  expect(t("similar.groups",{count:1},"de")).toBe("1 Gruppe");
  expect(t("similar.groups",{count:2},"de")).toBe("2 Gruppen");
  expect(t("similar.images",{count:1},"en")).toBe("1 image");
  expect(t("similar.images",{count:2},"en")).toBe("2 images");
});

test("dialog button ellipses follow the existing spaced majority", () => {
  for (const language of ["de", "en"] as const) {
    for (const key of ["action.compareDirs", "menu.view.settings", "space.button", "bookmarks.pin", "ssh.connections"] as const) {
      expect(t(key, {}, language)).toMatch(/ …$/);
    }
  }
});
