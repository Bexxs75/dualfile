// Reine Hilfen des Listers: Kodierungen, Zeilenerkennung, Hex-Zeilen, Suchbegriffe, Blob-Laden.
import type { DualfileApi } from "../../ipc/api";
import { MAX_BLOCK_BYTES, MAX_NEEDLE_BYTES, type FileInfo } from "../../ipc/types";

export const MAX_IMAGE_BYTES = 32 * 1024 * 1024;
export const MAX_MEDIA_BYTES = 128 * 1024 * 1024;
/** Größe eines Text-/Hex-Fensters, das die Oberfläche im Speicher hält (höchstens wenige MiB). */
export const TEXT_PAGE_BYTES = 256 * 1024;
export const HEX_PAGE_BYTES = 16 * 1024;
/** Bis hierher werden Zeilennummern gezählt (Zählen heißt, den Anfang der Datei zu lesen). */
export const LINE_SCAN_LIMIT = 64 * 1024 * 1024;
export const HEX_ROW = 16;
/** Breite der Hex-Spalte: 2 x (8 Bytes zu 23 Zeichen) plus zwei Leerzeichen dazwischen. */
export const HEX_WIDTH = 48;

export type TextEncodingId = "utf-8" | "utf-16le" | "utf-16be" | "windows-1252" | "iso-8859-1";
export const ENCODINGS: readonly TextEncodingId[] = ["utf-8", "utf-16le", "utf-16be", "windows-1252", "iso-8859-1"];
export type ViewMode = "text" | "hex" | "media";

export const ENCODING_LABEL: Record<TextEncodingId, string> = {
  "utf-8": "UTF-8",
  "utf-16le": "UTF-16 LE",
  "utf-16be": "UTF-16 BE",
  "windows-1252": "Windows-1252",
  "iso-8859-1": "ISO-8859-1",
};

// ---------------------------------------------------------------- Art und Modus

export const isImageInfo = (info: FileInfo): boolean => info.kind === "image" || info.mimeGuess.startsWith("image/");
export const isMediaInfo = (info: FileInfo): boolean =>
  info.kind === "audio" || info.kind === "video" || info.mimeGuess.startsWith("audio/") || info.mimeGuess.startsWith("video/");
export const canShowMedia = (info: FileInfo): boolean => isImageInfo(info) || isMediaInfo(info);

/** Automatische Wahl: Bild/Audio/Video → Medien, Text → Text, sonst Hex. */
export function autoMode(info: FileInfo): ViewMode {
  if (canShowMedia(info)) return "media";
  return info.kind === "text" ? "text" : "hex";
}

export function mediaLimit(info: FileInfo): number {
  return isImageInfo(info) ? MAX_IMAGE_BYTES : MAX_MEDIA_BYTES;
}

// ---------------------------------------------------------------- Kodierung

/** Erkennt eine Byte-Order-Mark am Dateianfang. */
export function detectBom(head: Uint8Array): { encoding: TextEncodingId; skip: number } | null {
  if (head[0] === 0xef && head[1] === 0xbb && head[2] === 0xbf) return { encoding: "utf-8", skip: 3 };
  if (head[0] === 0xff && head[1] === 0xfe) return { encoding: "utf-16le", skip: 2 };
  if (head[0] === 0xfe && head[1] === 0xff) return { encoding: "utf-16be", skip: 2 };
  return null;
}

const isWide = (encoding: TextEncodingId): boolean => encoding === "utf-16le" || encoding === "utf-16be";

/**
 * Dekodiert Bytes zu Text. Ungültige Bytes werden zu U+FFFD, nie zu einem Fehler. `atFileStart`:
 * nur dort wird eine Byte-Order-Mark entfernt. ISO-8859-1 ist hier echtes Latin-1 (der
 * `TextDecoder`-Name „iso-8859-1“ meint nach WHATWG Windows-1252).
 */
export function decodeBytes(bytes: Uint8Array, encoding: TextEncodingId, atFileStart = true): string {
  switch (encoding) {
    case "utf-8":
      return new TextDecoder("utf-8", { ignoreBOM: !atFileStart }).decode(bytes);
    case "utf-16le":
      return new TextDecoder("utf-16le", { ignoreBOM: !atFileStart }).decode(evenPart(bytes));
    case "utf-16be": {
      const even = evenPart(bytes);
      const swapped = new Uint8Array(even.length);
      for (let i = 0; i < even.length; i += 2) {
        swapped[i] = even[i + 1]!;
        swapped[i + 1] = even[i]!;
      }
      return new TextDecoder("utf-16le", { ignoreBOM: !atFileStart }).decode(swapped);
    }
    case "windows-1252":
      return new TextDecoder("windows-1252").decode(bytes);
    case "iso-8859-1": {
      let text = "";
      for (let i = 0; i < bytes.length; i += 4096) text += String.fromCharCode(...bytes.subarray(i, i + 4096));
      return text;
    }
  }
}

const evenPart = (bytes: Uint8Array): Uint8Array => (bytes.length % 2 === 0 ? bytes : bytes.subarray(0, bytes.length - 1));

/** Anzahl führender UTF-8-Folgebytes, die man überspringt, wenn ein Fenster mitten in einem Zeichen beginnt. */
export function leadingContinuation(bytes: Uint8Array, encoding: TextEncodingId): number {
  if (encoding !== "utf-8") return 0;
  let n = 0;
  while (n < 3 && n < bytes.length && (bytes[n]! & 0xc0) === 0x80) n += 1;
  return n;
}

/** Länge eines unvollständigen Zeichens am Ende (UTF-8: abgeschnittene Folge, UTF-16: halbes Zeichen). */
export function incompleteTail(bytes: Uint8Array, encoding: TextEncodingId): number {
  if (isWide(encoding)) return bytes.length % 2;
  if (encoding !== "utf-8") return 0;
  for (let back = 1; back <= 3 && back <= bytes.length; back += 1) {
    const b = bytes[bytes.length - back]!;
    if ((b & 0xc0) === 0x80) continue; // Folgebyte: weiter nach vorn
    const need = b >= 0xf0 ? 4 : b >= 0xe0 ? 3 : b >= 0xc0 ? 2 : 1;
    return need > back ? back : 0;
  }
  return 0;
}

// ---------------------------------------------------------------- Zeilen

const unit = (encoding: TextEncodingId): 1 | 2 => (isWide(encoding) ? 2 : 1);

function newlineAt(bytes: Uint8Array, i: number, encoding: TextEncodingId): boolean {
  if (encoding === "utf-16le") return bytes[i] === 0x0a && bytes[i + 1] === 0;
  if (encoding === "utf-16be") return bytes[i] === 0 && bytes[i + 1] === 0x0a;
  return bytes[i] === 0x0a;
}

export function countNewlines(bytes: Uint8Array, encoding: TextEncodingId): number {
  const step = unit(encoding);
  let count = 0;
  for (let i = 0; i + step <= bytes.length; i += step) if (newlineAt(bytes, i, encoding)) count += 1;
  return count;
}

/** Index hinter dem letzten Zeilenumbruch oder -1. */
export function lastLineEnd(bytes: Uint8Array, encoding: TextEncodingId): number {
  const step = unit(encoding);
  for (let i = bytes.length - (bytes.length % step) - step; i >= 0; i -= step) {
    if (newlineAt(bytes, i, encoding)) return i + step;
  }
  return -1;
}

/** Index hinter dem n-ten (ab 1) Zeilenumbruch oder -1, wenn es weniger gibt. */
export function nthLineEnd(bytes: Uint8Array, encoding: TextEncodingId, n: number): number {
  const step = unit(encoding);
  let seen = 0;
  for (let i = 0; i + step <= bytes.length; i += step) {
    if (newlineAt(bytes, i, encoding) && (seen += 1) === n) return i + step;
  }
  return -1;
}

/** Index hinter dem ersten Zeilenumbruch oder -1. */
export const firstLineEnd = (bytes: Uint8Array, encoding: TextEncodingId): number => nthLineEnd(bytes, encoding, 1);

/** Zerlegt Text in Zeilen (LF oder CRLF); ein Schlusszeilenumbruch erzeugt keine leere Endzeile. */
export function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n").map((line) => (line.endsWith("\r") ? line.slice(0, -1) : line));
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

// ---------------------------------------------------------------- Hex

export interface HexRow {
  offset: string;
  hex: string;
  ascii: string;
}

export const offsetDigits = (size: number): number => Math.max(8, Math.max(0, size).toString(16).length);

/** Eine Hex-Zeile: Offset, 16 Bytes in zwei Achtergruppen, ASCII. Die letzte Zeile ist kürzer, die Spalten bleiben ausgerichtet. */
export function formatHexRow(offset: number, bytes: Uint8Array, digits = 8): HexRow {
  const cells = [...bytes].map((b) => b.toString(16).padStart(2, "0"));
  const left = cells.slice(0, 8).join(" ");
  const right = cells.slice(8).join(" ");
  const hex = (cells.length > 8 ? `${left}  ${right}` : left).padEnd(HEX_WIDTH, " ");
  const ascii = [...bytes].map((b) => (b >= 0x20 && b <= 0x7e ? String.fromCharCode(b) : ".")).join("");
  return { offset: offset.toString(16).padStart(digits, "0"), hex, ascii };
}

export function hexRows(bytes: Uint8Array, startOffset: number, digits: number): HexRow[] {
  const rows: HexRow[] = [];
  for (let i = 0; i < bytes.length; i += HEX_ROW) {
    rows.push(formatHexRow(startOffset + i, bytes.subarray(i, i + HEX_ROW), digits));
  }
  return rows;
}

// ---------------------------------------------------------------- Eingaben

/** „0x1f“ (hex) oder „1234“ (dezimal); null bei ungültiger Eingabe. */
export function parseOffset(input: string): number | null {
  const text = input.trim().toLowerCase();
  if (/^0x[0-9a-f]+$/.test(text)) return Number.parseInt(text.slice(2), 16);
  if (/^\d+$/.test(text)) return Number.parseInt(text, 10);
  return null;
}

export type TextGoto = { kind: "line"; line: number } | { kind: "percent"; percent: number };

/** „120“ = Zeile 120, „40%“ = 40 Prozent der Datei. */
export function parseTextGoto(input: string): TextGoto | null {
  const text = input.trim();
  const percent = /^(\d+(?:[.,]\d+)?)\s*%$/.exec(text);
  if (percent) {
    const value = Number.parseFloat(percent[1]!.replace(",", "."));
    return value >= 0 && value <= 100 ? { kind: "percent", percent: value } : null;
  }
  if (/^\d+$/.test(text)) {
    const line = Number.parseInt(text, 10);
    return line >= 1 ? { kind: "line", line } : null;
  }
  return null;
}

/** „de ad BE EF“, „0xDE,0xAD“ oder „deadbeef“ → Bytes; null, wenn keine gerade Zahl Hexziffern. */
export function parseHexBytes(input: string): Uint8Array | null {
  const digits = input.replace(/0x/gi, "").replace(/[\s,;:]/g, "");
  if (digits === "" || digits.length % 2 !== 0 || !/^[0-9a-f]+$/i.test(digits)) return null;
  const out = new Uint8Array(digits.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = Number.parseInt(digits.slice(i * 2, i * 2 + 2), 16);
  return out;
}

let cp1252: Map<string, number> | null = null;
function windows1252Byte(char: string): number | null {
  const code = char.charCodeAt(0);
  if (code < 0x80 || (code >= 0xa0 && code <= 0xff)) return code;
  if (cp1252 === null) {
    cp1252 = new Map();
    const decoder = new TextDecoder("windows-1252");
    for (let b = 0x80; b < 0xa0; b += 1) cp1252.set(decoder.decode(Uint8Array.of(b)), b);
  }
  return cp1252.get(char) ?? null;
}

/** Suchtext → Bytes in der angezeigten Kodierung; null, wenn nicht darstellbar oder zu lang. */
export function encodeNeedle(text: string, encoding: TextEncodingId): Uint8Array | null {
  if (text === "") return null;
  let out: Uint8Array;
  if (encoding === "utf-8") out = Uint8Array.from(new TextEncoder().encode(text));
  else if (isWide(encoding)) {
    out = new Uint8Array(text.length * 2);
    for (let i = 0; i < text.length; i += 1) {
      const code = text.charCodeAt(i);
      out[i * 2] = encoding === "utf-16le" ? code & 0xff : code >> 8;
      out[i * 2 + 1] = encoding === "utf-16le" ? code >> 8 : code & 0xff;
    }
  } else {
    const bytes: number[] = [];
    for (const char of text) {
      const b = encoding === "iso-8859-1" ? (char.charCodeAt(0) <= 0xff && char.length === 1 ? char.charCodeAt(0) : null) : windows1252Byte(char);
      if (b === null) return null;
      bytes.push(b);
    }
    out = Uint8Array.from(bytes);
  }
  return out.length > MAX_NEEDLE_BYTES ? null : out;
}

export function percentOf(position: number, size: number): number {
  if (size <= 0) return 100;
  return Math.min(100, Math.max(0, Math.round((position / size) * 100)));
}

// ---------------------------------------------------------------- Laden

export interface Cancel {
  cancelled: boolean;
}

/** Liest die ganze Datei in Blöcken zu einem Blob; null, wenn zwischendurch abgebrochen wurde. */
export async function loadBlob(api: DualfileApi, path: string, size: number, mime: string, cancel: Cancel): Promise<Blob | null> {
  const parts: Uint8Array<ArrayBuffer>[] = [];
  let offset = 0;
  while (offset < size) {
    const block = await api.readBlock(path, offset, Math.min(MAX_BLOCK_BYTES, size - offset));
    if (cancel.cancelled) return null;
    if (block.length === 0) break;
    parts.push(new Uint8Array(block));
    offset += block.length;
  }
  return new Blob(parts, { type: mime });
}

/** Liest `len` Bytes ab `offset` (auch über mehrere Blöcke). */
export async function readRange(api: DualfileApi, path: string, offset: number, len: number): Promise<Uint8Array> {
  const out = new Uint8Array(len);
  let filled = 0;
  while (filled < len) {
    const block = await api.readBlock(path, offset + filled, Math.min(MAX_BLOCK_BYTES, len - filled));
    if (block.length === 0) break;
    out.set(block, filled);
    filled += block.length;
  }
  return filled === len ? out : out.subarray(0, filled);
}
