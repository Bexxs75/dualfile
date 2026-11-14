import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import de from "../../i18n/de";
import en from "../../i18n/en";
import type { FileInfo } from "../../ipc/types";
import { blobHolder } from "./blobHolder";
import {
  MAX_IMAGE_BYTES,
  MAX_MEDIA_BYTES,
  autoMode,
  canShowMedia,
  countNewlines,
  decodeBytes,
  detectBom,
  encodeNeedle,
  firstLineEnd,
  formatHexRow,
  hexRows,
  incompleteTail,
  lastLineEnd,
  leadingContinuation,
  mediaLimit,
  nthLineEnd,
  offsetDigits,
  parseHexBytes,
  parseOffset,
  parseTextGoto,
  percentOf,
  splitLines,
} from "./lib";

const bytes = (...values: number[]): Uint8Array => Uint8Array.from(values);
const ascii = (text: string): number[] => [...text].map((c) => c.charCodeAt(0));
const info = (kind: FileInfo["kind"], mimeGuess = "application/octet-stream", size = 10): FileInfo => ({ size, kind, modified: null, mimeGuess });

describe("Dekodierung", () => {
  test("UTF-8 zeigt Umlaute und entfernt die Byte-Order-Mark am Dateianfang", () => {
    const text = new TextEncoder().encode("Grüße €");
    expect(decodeBytes(text, "utf-8")).toBe("Grüße €");
    expect(decodeBytes(bytes(0xef, 0xbb, 0xbf, ...text), "utf-8", true)).toBe("Grüße €");
  });

  test("UTF-8: mitten in der Datei bleibt eine Marke stehen (nur der Anfang hat eine)", () => {
    expect(decodeBytes(bytes(0xef, 0xbb, 0xbf, 0x41), "utf-8", false)).toBe("﻿A");
  });

  test("UTF-16 LE mit Byte-Order-Mark", () => {
    expect(decodeBytes(bytes(0xff, 0xfe, 0x48, 0x00, 0xfc, 0x00, 0xac, 0x20), "utf-16le")).toBe("Hü€");
  });

  test("UTF-16 BE mit Byte-Order-Mark", () => {
    expect(decodeBytes(bytes(0xfe, 0xff, 0x00, 0x48, 0x00, 0xfc, 0x20, 0xac), "utf-16be")).toBe("Hü€");
  });

  test("UTF-16 ignoriert ein halbes Zeichen am Ende, statt zu scheitern", () => {
    expect(decodeBytes(bytes(0x48, 0x00, 0x49), "utf-16le", false)).toBe("H");
  });

  test("Windows-1252: 0x80 ist das Euro-Zeichen", () => {
    expect(decodeBytes(bytes(0x80, 0xe4), "windows-1252")).toBe("€ä");
  });

  test("ISO-8859-1: 0x80 bleibt ein Steuerzeichen (nicht Windows-1252)", () => {
    expect(decodeBytes(bytes(0x80, 0xe4), "iso-8859-1")).toBe("\u0080ä");
  });

  test("ungültige UTF-8-Bytes werden zu Ersatzzeichen, kein Fehler", () => {
    expect(decodeBytes(bytes(...ascii("a"), 0xff, ...ascii("b"), 0xc3), "utf-8")).toBe("a�b�");
  });

  test("ein einzelnes Surrogat in UTF-16 wird zum Ersatzzeichen", () => {
    expect(decodeBytes(bytes(0x00, 0xd8, 0x41, 0x00), "utf-16le", false)).toBe("�A");
  });

  test("lange ISO-8859-1-Blöcke werden ohne Stapelüberlauf dekodiert", () => {
    expect(decodeBytes(new Uint8Array(300_000).fill(0x41), "iso-8859-1")).toHaveLength(300_000);
  });
});

describe("Byte-Order-Mark und Zeichengrenzen", () => {
  test("erkennt UTF-8, UTF-16 LE und BE, sonst nichts", () => {
    expect(detectBom(bytes(0xef, 0xbb, 0xbf, 0x41))).toEqual({ encoding: "utf-8", skip: 3 });
    expect(detectBom(bytes(0xff, 0xfe, 0x41, 0))).toEqual({ encoding: "utf-16le", skip: 2 });
    expect(detectBom(bytes(0xfe, 0xff, 0, 0x41))).toEqual({ encoding: "utf-16be", skip: 2 });
    expect(detectBom(bytes(0x41, 0x42))).toBeNull();
  });

  test("ein Fenster, das mitten in einem UTF-8-Zeichen beginnt, überspringt die Folgebytes", () => {
    expect(leadingContinuation(bytes(0xbc, 0x41), "utf-8")).toBe(1);
    expect(leadingContinuation(bytes(0x41), "utf-8")).toBe(0);
    expect(leadingContinuation(bytes(0xbc), "windows-1252")).toBe(0);
  });

  test("unvollständiges Zeichen am Ende: UTF-8 und UTF-16", () => {
    expect(incompleteTail(bytes(0x41, 0xc3), "utf-8")).toBe(1);
    expect(incompleteTail(bytes(0x41, 0xe2, 0x82), "utf-8")).toBe(2);
    expect(incompleteTail(bytes(0x41, 0xc3, 0xbc), "utf-8")).toBe(0);
    expect(incompleteTail(bytes(0x41, 0x00, 0x42), "utf-16le")).toBe(1);
    expect(incompleteTail(bytes(0x41), "iso-8859-1")).toBe(0);
  });
});

describe("Zeilen", () => {
  test("zählt Zeilenumbrüche in 8-Bit- und 16-Bit-Kodierungen", () => {
    expect(countNewlines(bytes(...ascii("a\nb\nc")), "utf-8")).toBe(2);
    expect(countNewlines(bytes(0x0a, 0x00, 0x41, 0x00, 0x0a, 0x00), "utf-16le")).toBe(2);
    expect(countNewlines(bytes(0x00, 0x0a, 0x00, 0x41), "utf-16be")).toBe(1);
    // 0x0a als Teil eines anderen UTF-16-Zeichens (U+0A41) ist kein Zeilenumbruch
    expect(countNewlines(bytes(0x0a, 0x0a, 0x41, 0x0a), "utf-16le")).toBe(0);
  });

  test("Zeilenenden: letztes, erstes und n-tes", () => {
    const data = bytes(...ascii("ab\ncd\nef"));
    expect(lastLineEnd(data, "utf-8")).toBe(6);
    expect(firstLineEnd(data, "utf-8")).toBe(3);
    expect(nthLineEnd(data, "utf-8", 2)).toBe(6);
    expect(nthLineEnd(data, "utf-8", 3)).toBe(-1);
    expect(lastLineEnd(bytes(...ascii("kein umbruch")), "utf-8")).toBe(-1);
  });

  test("Zeilen werden an LF und CRLF getrennt; die Schlusszeile erzeugt keine leere Zeile", () => {
    expect(splitLines("a\r\nb\nc\n")).toEqual(["a", "b", "c"]);
    expect(splitLines("a\n\nb")).toEqual(["a", "", "b"]);
    expect(splitLines("")).toEqual([]);
  });
});

describe("Hex-Zeilen", () => {
  test("volle Zeile: Offset, zwei Achtergruppen, ASCII", () => {
    const row = formatHexRow(0x10, bytes(...ascii("Hello, World!!!!")));
    expect(row.offset).toBe("00000010");
    expect(row.hex).toBe("48 65 6c 6c 6f 2c 20 57  6f 72 6c 64 21 21 21 21");
    expect(row.ascii).toBe("Hello, World!!!!");
  });

  test("Steuerzeichen und Bytes ab 0x7f erscheinen als Punkt", () => {
    expect(formatHexRow(0, bytes(0x00, 0x1f, 0x41, 0x7f, 0xff)).ascii).toBe("..A..");
  });

  test("die letzte Zeile ist kürzer, die Hex-Spalte bleibt gleich breit", () => {
    const full = formatHexRow(0, new Uint8Array(16));
    const short = formatHexRow(16, bytes(1, 2, 3));
    expect(short.hex.trimEnd()).toBe("01 02 03");
    expect(short.hex).toHaveLength(full.hex.length);
    expect(short.ascii).toHaveLength(3);
  });

  test("mehr als acht, aber weniger als 16 Bytes: Lücke nach dem achten", () => {
    expect(formatHexRow(0, bytes(1, 2, 3, 4, 5, 6, 7, 8, 9)).hex.trimEnd()).toBe("01 02 03 04 05 06 07 08  09");
  });

  test("hexRows teilt in 16er-Zeilen mit fortlaufenden Offsets ab dem Fensteranfang", () => {
    const rows = hexRows(new Uint8Array(40), 0x1000, 8);
    expect(rows.map((r) => r.offset)).toEqual(["00001000", "00001010", "00001020"]);
    expect(rows[2]!.hex.trimEnd().split(/\s+/)).toHaveLength(8);
  });

  test("Offsets haben mindestens 8 Stellen, bei Dateien über 4 GiB mehr", () => {
    expect(offsetDigits(100)).toBe(8);
    expect(offsetDigits(0x1_0000_0000)).toBe(9);
    expect(formatHexRow(0x1_0000_0000, bytes(1), 9).offset).toBe("100000000");
  });
});

describe("Eingaben", () => {
  test("Offset: dezimal oder 0x-hexadezimal, sonst ungültig", () => {
    expect(parseOffset("255")).toBe(255);
    expect(parseOffset(" 0xFF ")).toBe(255);
    expect(parseOffset("ff")).toBeNull();
    expect(parseOffset("-1")).toBeNull();
    expect(parseOffset("")).toBeNull();
  });

  test("Sprung im Text: Zeile oder Prozent", () => {
    expect(parseTextGoto("120")).toEqual({ kind: "line", line: 120 });
    expect(parseTextGoto("40%")).toEqual({ kind: "percent", percent: 40 });
    expect(parseTextGoto("12,5 %")).toEqual({ kind: "percent", percent: 12.5 });
    expect(parseTextGoto("0")).toBeNull();
    expect(parseTextGoto("101%")).toBeNull();
    expect(parseTextGoto("abc")).toBeNull();
  });

  test("Bytefolge aus Hex-Schreibweisen", () => {
    expect(parseHexBytes("de ad BE ef")).toEqual(bytes(0xde, 0xad, 0xbe, 0xef));
    expect(parseHexBytes("0xDE,0xAD")).toEqual(bytes(0xde, 0xad));
    expect(parseHexBytes("deadbeef")).toEqual(bytes(0xde, 0xad, 0xbe, 0xef));
    expect(parseHexBytes("abc")).toBeNull();
    expect(parseHexBytes("zz")).toBeNull();
    expect(parseHexBytes("")).toBeNull();
  });

  test("Suchtext wird in der angezeigten Kodierung kodiert", () => {
    expect(encodeNeedle("ü", "utf-8")).toEqual(bytes(0xc3, 0xbc));
    expect(encodeNeedle("ü", "utf-16le")).toEqual(bytes(0xfc, 0x00));
    expect(encodeNeedle("ü", "utf-16be")).toEqual(bytes(0x00, 0xfc));
    expect(encodeNeedle("ü€", "windows-1252")).toEqual(bytes(0xfc, 0x80));
    expect(encodeNeedle("ü", "iso-8859-1")).toEqual(bytes(0xfc));
  });

  test("nicht darstellbare, leere oder zu lange Suchtexte ergeben null", () => {
    expect(encodeNeedle("€", "iso-8859-1")).toBeNull();
    expect(encodeNeedle("日", "windows-1252")).toBeNull();
    expect(encodeNeedle("", "utf-8")).toBeNull();
    expect(encodeNeedle("x".repeat(4097), "utf-8")).toBeNull();
  });

  test("Prozent ist auf 0 bis 100 begrenzt, eine leere Datei steht bei 100", () => {
    expect(percentOf(50, 200)).toBe(25);
    expect(percentOf(500, 200)).toBe(100);
    expect(percentOf(0, 0)).toBe(100);
  });
});

describe("Moduswahl", () => {
  test("Text → Text, Binär → Hex, Bild/Audio/Video → Medien", () => {
    expect(autoMode(info("text", "text/plain"))).toBe("text");
    expect(autoMode(info("binary"))).toBe("hex");
    expect(autoMode(info("image", "image/png"))).toBe("media");
    expect(autoMode(info("audio", "audio/wav"))).toBe("media");
    expect(autoMode(info("video", "video/mp4"))).toBe("media");
  });

  test("SVG ist für den Kern ein Bild (Text mit Bild-Typ), also Medienansicht", () => {
    expect(autoMode(info("text", "image/svg+xml"))).toBe("media");
    expect(canShowMedia(info("text", "text/plain"))).toBe(false);
    expect(canShowMedia(info("binary"))).toBe(false);
  });

  test("Obergrenzen: Bild 32 MiB, Audio/Video 128 MiB", () => {
    expect(MAX_IMAGE_BYTES).toBe(32 * 1024 * 1024);
    expect(MAX_MEDIA_BYTES).toBe(128 * 1024 * 1024);
    expect(mediaLimit(info("image", "image/png"))).toBe(MAX_IMAGE_BYTES);
    expect(mediaLimit(info("text", "image/svg+xml"))).toBe(MAX_IMAGE_BYTES);
    expect(mediaLimit(info("audio", "audio/mpeg"))).toBe(MAX_MEDIA_BYTES);
    expect(mediaLimit(info("video", "video/mp4"))).toBe(MAX_MEDIA_BYTES);
  });
});

describe("blobHolder", () => {
  let counter = 0;
  let live: Set<string>;
  beforeEach(() => {
    counter = 0;
    live = new Set();
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL: vi.fn(() => {
        const url = `blob:test/${(counter += 1)}`;
        live.add(url);
        return url;
      }),
      revokeObjectURL: vi.fn((url: string) => void live.delete(url)),
    });
    blobHolder.releaseAll();
  });
  afterEach(() => {
    blobHolder.releaseAll();
    vi.unstubAllGlobals();
  });

  test("ein neuer Blob gibt den alten frei, es gibt nie zwei aktive", () => {
    const first = blobHolder.acquire(new Blob(["a"]));
    expect(live.size).toBe(1);
    const second = blobHolder.acquire(new Blob(["b"]));
    expect(URL.revokeObjectURL).toHaveBeenCalledWith(first.url);
    expect([...live]).toEqual([second.url]);
    expect(blobHolder.active()).toBe(second.url);
  });

  test("release gibt frei; danach ist nichts aktiv", () => {
    const lease = blobHolder.acquire(new Blob(["a"]));
    lease.release();
    expect(live.size).toBe(0);
    expect(blobHolder.active()).toBeNull();
  });

  test("ein veralteter Besitzer gibt den neuen Blob nicht frei", () => {
    const old = blobHolder.acquire(new Blob(["a"]));
    const fresh = blobHolder.acquire(new Blob(["b"]));
    old.release();
    expect(blobHolder.active()).toBe(fresh.url);
    expect(live.has(fresh.url)).toBe(true);
  });

  test("doppeltes release ruft revokeObjectURL nur einmal auf", () => {
    const lease = blobHolder.acquire(new Blob(["a"]));
    lease.release();
    lease.release();
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
  });

  test("releaseAll ohne aktiven Blob tut nichts", () => {
    blobHolder.releaseAll();
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();
  });
});

describe("Übersetzungen des Listers", () => {
  const keys = (dictionary: Record<string, string>): string[] => Object.keys(dictionary).filter((k) => k.startsWith("lister.")).sort();
  test("Deutsch und Englisch haben dieselben lister.*-Schlüssel", () => {
    expect(keys(en)).toEqual(keys(de));
    expect(keys(de).length).toBeGreaterThan(30);
  });
  test("die neuen Schlüssel für Hinweis, Menü und Tooltip gibt es in beiden Sprachen", () => {
    for (const key of ["toast.viewFolder", "menu.view.quickView", "tip.quickView"]) {
      expect(de).toHaveProperty(key);
      expect(en).toHaveProperty(key);
    }
  });
});
