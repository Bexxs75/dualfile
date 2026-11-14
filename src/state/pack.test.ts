import { describe, expect, test } from "vitest";
import { PACK_FORMATS } from "../ipc/types";
import { PACK_EXTENSIONS, stripPackExtension, suggestPackBase, withPackExtension } from "./pack";

describe("Packen: Namen", () => {
  test("jedes Format hat eine Endung", () => {
    expect(PACK_FORMATS.map((f) => PACK_EXTENSIONS[f])).toEqual([".zip", ".tar.gz", ".tar.bz2", ".tar.xz", ".7z"]);
  });
  test("Vorschlag: Datei ohne Endung, Ordner und Punktdatei unverändert, leer → Archiv", () => {
    expect(suggestPackBase("foto.jpg", false)).toBe("foto");
    expect(suggestPackBase("daten.v2.txt", false)).toBe("daten.v2");
    expect(suggestPackBase("Bilder", true)).toBe("Bilder");
    expect(suggestPackBase("ordner.d", true)).toBe("ordner.d");
    expect(suggestPackBase(".bashrc", false)).toBe(".bashrc");
    expect(suggestPackBase(undefined, false)).toBe("Archiv");
  });
  test("Endung folgt dem Format und ersetzt eine vorhandene", () => {
    expect(withPackExtension("foto", "zip")).toBe("foto.zip");
    expect(withPackExtension("foto.zip", "tarGz")).toBe("foto.tar.gz");
    expect(withPackExtension("foto.tar.gz", "sevenZ")).toBe("foto.7z");
    expect(withPackExtension("foto.tar.xz", "tarBz2")).toBe("foto.tar.bz2");
    expect(withPackExtension("a.b", "zip")).toBe("a.b.zip");
  });
  test("stripPackExtension lässt eine nackte Endung stehen", () => {
    expect(stripPackExtension(".zip")).toBe(".zip");
    expect(stripPackExtension("x.TAR.GZ")).toBe("x");
  });
});
