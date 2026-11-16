import { createTerminalMock } from "./terminalMock";
import { mockSystemPlaces, systemPlaceProfile, systemPlaceExists } from "./systemPlaces";
import { createSimilarMock } from "./similarMock";
import { createSpaceMock } from "./spaceMock";
// Speicherinterne Attrappe der `DualfileApi`: Dateibaum im Arbeitsspeicher, Aufträge mit
// simuliertem Fortschritt über einen austauschbaren Zeitgeber. Sie verhält sich wie der Kern:
// nie stilles Überschreiben, endgültiges Löschen nur mit Token, Fehler pro Element.
import { createMockSshPrompts } from "./mock-ssh-prompt";
import { CoreError, type DualfileApi } from "./api";
import { diffBytes } from "./mock-diff";
import { compilePatterns, scanText } from "./mock-search";
import { createMockPlugins, type MockPluginDef, type PluginHost } from "./mock-plugins";
import {
  COMPARE_STATES,
  DIFF_MAX_BYTES,
  MAX_BLOCK_BYTES,
  MAX_COMPARE_FILTER_BYTES,
  MAX_COMPARE_PAGE,
  MAX_COMPARES,
  MAX_SEARCH_HITS,
  MAX_SEARCH_PAGE,
  MAX_SEARCHES,
  MAX_SYNC_PLANS,
  MSG_DIFF_TOO_LARGE,
  PACK_FORMATS,
  SYNC_PLAN_PREVIEW,
  MAX_NEEDLE_BYTES,
  MAX_REGEX_NAME_BYTES,
  MAX_REGEX_NAMES,
  MAX_REGEX_PATTERN_CHARS,
  MAX_REGEX_RESULT_BYTES,
} from "./types";
import type {
  CompareCounts,
  CompareEntry,
  CompareMode,
  CompareOptions,
  ComparePage,
  CompareSide,
  CompareState,
  ConflictAnswer,
  ConnectOutcome,
  ConnectionEntry,
  ConnectionProfile,
  ConnectionProtocol,
  UndoEntry,
  UndoResult,
  UndoKind,
  UndoErrorKind,
  CoreEvent,
  DeletePlan,
  Entry,
  EntryKind,
  FileInfo,
  FileKind,
  MediaTags,
  JobStatus,
  JobTitle,
  Listing,
  RegexResult,
  RenameItem,
  Resolution,
  PinInfo,
  SecretStoreKind,
  TrustDecision,
  DiffResult,
  SearchHit,
  SearchOptions,
  SearchPage,
  SearchResult,
  SearchStopReason,
  SearchSummary,
  Settings,
  SyncAction,
  SyncOptions,
  SyncActionKind,
  SyncMode,
  SyncPlan,
  SyncPlanNote,
  SyncPlanPage,
} from "./types";

/** Zeitgeber, den Tests durch eine von Hand gesteuerte Fassung ersetzen. */
export interface MockScheduler {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface MockOptions {
  itemError?: boolean;
  /** Simulate destination corruption after a verified write. */
  verifyFailure?: boolean;
  /** Persönlicher Ordner, Standard `/home/demo`. */
  home?: string;
  scheduler?: MockScheduler;
  /** Uhr für neu angelegte Einträge (Millisekunden seit 1970). */
  now?: () => number;
  initialSettings?: Settings;
  /**
   * Legt im persönlichen Ordner `Ansicht/` Beispieldateien für den Lister an (echte PNG-/WAV-Dateien,
   * SVG, Text in mehreren Kodierungen, Binärdatei, sehr große Text-, Bild- und Videodatei). Standard
   * aus, damit die übrigen Ordner und Tests unverändert bleiben.
   */
  samples?: boolean;
  /**
   * Legt im persönlichen Ordner `Vergleich/` Demo-Ordner (`links`, `rechts`) für den Ordnervergleich an
   * (alle Zustände, ein nicht lesbarer Teilbaum) und `text/` mit Dateien für den Textvergleich.
   * Standard: wie `samples`.
   */
  compareDemo?: boolean;
  /** Schlüsselbund verfügbar (Standard ja); sonst landen Geheimnisse nur „im Speicher der Sitzung“. */
  keychain?: boolean;
  /** Zusätzliche Mock-Plugins neben den Demo-Plugins (z. B. für Tests mit unsauberen Texten). */
  plugins?: MockPluginDef[];
}

/** Die Mock-API plus Schalter für Tests und den Entwurf. */
export interface MockApi extends DualfileApi {
  requestClose(): void;
  requestSshPrompt(text: string): Promise<string | null>;
  closeSshPrompts(): void;
  /** Lässt eine Verbindung abreißen (Verbindung verloren), ohne dass jemand trennt. */
  dropConnection(connectionId: string): void;
}

/** Dauer eines simulierten Schritts (ein Block einer Datei). */
export const MOCK_STEP_MS = 40;
const BYTES_PER_SECOND = 50_000_000;
const CHUNK_BYTES = 1_000_000;
const MAX_CHUNKS = 4;

interface Node {
  entry: Entry;
  children?: Map<string, Node>;
  /** Nur bei Archivdateien: der Inhalt als Ordnerbaum (Wurzel ohne Namen). */
  archive?: Node;
  /** Inhaltsstand einer Datei (Standard 0): gleiche Größe, aber anderer Stand = anderer Inhalt. */
  version?: number;
}

/**
 * Streckenlimit je Suchaufruf im Mock (der Kern: 256 MiB). Bewusst klein, damit „weitersuchen“
 * (`continue`) auch auf den kleinen Beispieldateien und in Tests erreichbar ist.
 */
export const MOCK_SEARCH_LIMIT = 4 * 1024 * 1024;
/**
 * Grenzen der Mehrdateien-Suche im Mock (der Kern: 64 MiB je Datei, 8 GiB gesamt). Bewusst klein, damit
 * `incomplete` an den Beispieldateien (150-MiB-Textdatei) und in Tests erreichbar ist.
 */
export const MOCK_SEARCH_FILE_BYTES = 1024 * 1024;
export const MOCK_SEARCH_TOTAL_BYTES = 16 * 1024 * 1024;
/** Server und Plugins nur mit kleinem Budget (wie `SearchLimits::for_remote`). */
export const MOCK_SEARCH_REMOTE_FILES = 2_000;

// ---------------------------------------------------------------- Inhalte der Beispieldateien

const LINE_BYTES = 64;
const encoder = new TextEncoder();
const TEXT_EXTENSIONS = new Set(["txt", "md", "sh", "toml", "json", "ts", "rs", "csv"]);
/** Kopfbytes (Signatur) je Endung, damit die Typerkennung wie beim Kern arbeitet. */
const MAGIC: Record<string, number[]> = {
  mp4: [0, 0, 0, 0x18, ...encoder.encode("ftypmp42")],
  png: [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
  jpg: [0xff, 0xd8, 0xff, 0xe0],
  jpeg: [0xff, 0xd8, 0xff, 0xe0],
  gif: [...encoder.encode("GIF89a")],
  pdf: [...encoder.encode("%PDF-1.7\n")],
  gz: [0x1f, 0x8b, 0x08],
  appimage: [0x7f, 0x45, 0x4c, 0x46],
  ods: [0x50, 0x4b, 0x03, 0x04],
};


// ---------------------------------------------------------------- Beispieldateien für den Lister

const fromBase64 = (text: string): Uint8Array => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

function utf16(text: string, bigEndian: boolean): Uint8Array {
  const out = new Uint8Array(2 + text.length * 2);
  out.set(bigEndian ? [0xfe, 0xff] : [0xff, 0xfe]);
  for (let i = 0; i < text.length; i += 1) {
    const unit = text.charCodeAt(i);
    out[2 + i * 2] = bigEndian ? unit >> 8 : unit & 0xff;
    out[3 + i * 2] = bigEndian ? unit & 0xff : unit >> 8;
  }
  return out;
}

const UMLAUT_TEXT = "Grüße aus Köln\nÄÖÜ äöü ß € – Zeile mit Umlauten\nDritte Zeile: Beispieltext für den Lister\n";
const SVG_TEXT =
  '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80" viewBox="0 0 120 80">' +
  '<rect width="120" height="80" rx="10" fill="#e0a45c"/><circle cx="60" cy="40" r="24" fill="#2b3b4d"/></svg>\n';

/** Feste Inhalte (Name → Bytes); alles andere erzeugt `mockFileBytes` aus Größe und Endung. */
const SAMPLE_FILES: Record<string, Uint8Array> = {
  "umlaute.txt": encoder.encode(UMLAUT_TEXT),
  "utf16.txt": utf16(UMLAUT_TEXT, false),
  "utf16-be.txt": utf16(UMLAUT_TEXT, true),
  "windows1252.txt": Uint8Array.from([...encoder.encode("Gr"), 0xfc, 0xdf, ...encoder.encode("e, Preis 5 "), 0x80, 0x0a]),
  "muster.png": fromBase64("iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAACX0lEQVR42gXBQacrOwAA4Pmhs5xFXHHFEUdUVFRUVFRUVFRUVFRUVFQcccQ1nlnMYhazmMUs56e872s2367Pdg7t+Gr72JZ3m1IbPq3LrflpVWnFb8tqS/61qG/Bf22zuW59dLPvxmfXh668uhS78O5c6synU7kTPx0rHfntUO3Av67ZLFzvcHZwfMDew/KEKcDwgi5C84YqQfGBLEPyA1GB4Bc2m8HrDc8Wj3fcO1weOHkcntgFbF5YRSzemCVMPhhlDH5ws2m6Xuls6HijvaXlTpOj4UGdp+ZJVaDiRVmk5E1RouBDm03x9cJnzccr7w0vN54sD3fuHDcPrjwXT84CJy+OIgdv3mxSrmc5KzleZK9lucpkZLhJZ6W5S+WkeEjmJXlKFCR4yWYTej3pWerxrHuly0UnrcNVO6PNTSurxV0zp8lDI6/BUzcbt+vRzsKOJ9tLW842KRsu1mlrrlYZK26WWUvuFjkLHrbZmF8PfuZ+PPpe+HLySfpw9k55c/FKe3H1zHhy88h6cPfNRuO6jzOL4yH2PJZjTCKGU3QymnNUKopLZDqSa0QmgltsNpLXXZ5pHve5Z7kccuI5HLMT2ZyyklmcM1OZXDLSGVxzs+G6fteZ1HFXe1rLviZWw6E6Xs2xKlHFqTJZybkiVcGlNhsa1q9hxsP4PfRkKLsh0SHsB8cGcxgUH8RxYGIgpwHJAZyHZoPT+nea0TR+TT2eyveUyBR2k6OT2U+KTeIwMT6R44TEBE5Ts4Fl/bPMcBn/Lj1ayteS8BK+F0cWs1sUXcR+YWwhhwXxBRyX/wF5Z9EQZZ9IhAAAAABJRU5ErkJggg=="),
  "logo.svg": encoder.encode(SVG_TEXT),
  "klick.wav": fromBase64("UklGRmQGAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YUAGAACAlKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppSAa1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlH9rWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWt/lKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUf2tZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlrgJSms7q7tKeWgW1bTUVESlZofJCjsbq7tqqZhXFeT0ZESFRkeI2gr7m7t62ciXRhUUdER1FhdImcrbe7ua+gjXhkVEhERk9ecYWZqra7urGjkHxoVkpERU1bbYGWp7S7urOmlH9rWUxFREtYaX6SpLK6u7Wpl4NvXE5FRElVZnqOobC5u7erm4dyX1BGREhSY3aLnq64vLiunot2Y1JIREZQX3KHm6u3u7mwoY56ZlVJREVOXG+Dl6m1u7qypJJ+aVhLREVMWWt/lKazuru0p5aBbVtNRURKVmh8kKOxuru2qpmFcV5PRkRIVGR4jaCvubu3rZyJdGFRR0RHUWF0iZytt7u5r6CNeGRUSERGT15xhZmqtru6saOQfGhWSkRFTVttgZantLu6s6aUgGtZTEVES1hpfpKksrq7tamXg29cTkVESVVmeo6hsLm7t6ubh3JfUEZESFJjdouerri8uK6ei3ZjUkhERlBfcoebq7e7ubChjnpmVUlERU5cb4OXqbW7urKkkn5pWEtERUxZa4CUprO6u7SnloFtW01FREpWaHyQo7G6u7aqmYVxXk9GREhUZHiNoK+5u7etnIl0YVFHREdRYXSJnK23u7mvoI14ZFRIREZPXnGFmaq2u7qxo5B8aFZKREVNW22Blqe0u7qzppR/a1lMRURLWGl+kqSyuru1qZeDb1xORURJVWZ6jqGwubu3q5uHcl9QRkRIUmN2i56uuLy4rp6LdmNSSERGUF9yh5urt7u5sKGOemZVSURFTlxvg5eptbu6sqSSfmlYS0RFTFlr"),
  "zufall.bin": Uint8Array.from({ length: 700 }, (_, i) => (i * 7 + (i >> 4)) & 0xff),
};
/** Texte für den Textvergleich der Demo (`Vergleich/text/`); nicht Teil von `Ansicht/`. */
const COMPARE_FILES: Record<string, Uint8Array> = {
  "vergleich-alt.txt": encoder.encode(
    "Rezept: Pfannkuchen\n\nZutaten:\n- 250 g Mehl\n- 2 Eier\n- 500 ml Milch\n- 1 Prise Salz\n\nZubereitung:\nAlles verrühren und in der Pfanne ausbacken.\nMit Zucker servieren.\n",
  ),
  "vergleich-neu.txt": encoder.encode(
    "Rezept: Pfannkuchen\n\nZutaten:\n- 300 g Mehl\n- 2 Eier\n- 500 ml Milch\n- 1 Prise Salz\n- 1 EL Zucker\n\nZubereitung:\nAlles verrühren und 10 Minuten ruhen lassen.\nIn der Pfanne ausbacken.\nMit Zucker servieren",
  ),
  "vergleich-bin.dat": Uint8Array.from([0x44, 0x46, 0x00, 0x01, 0x02, 0xff, 0x00, 0x10]),
};

const SAMPLE_BIG: [string, number][] = [
  ["riesig.txt", 150 * 1024 * 1024], // Text, blockweise
  ["panorama.png", 40 * 1024 * 1024], // über der Bildgrenze von 32 MiB
  ["film.mp4", 200 * 1024 * 1024], // über der Mediengrenze von 128 MiB
];

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

/** Textdatei: bekannte Textendung oder reine Punktdatei wie `.bashrc`. */
const isTextName = (name: string): boolean =>
  TEXT_EXTENSIONS.has(extensionOf(name)) || (name.startsWith(".") && !name.slice(1).includes("."));

function textLine(index: number, name: string): Uint8Array {
  const ascii = name.replace(/[^\x20-\x7e]/g, "?");
  const text = `Zeile ${String(index).padStart(8, "0")}: Beispieltext zu ${ascii}`.padEnd(LINE_BYTES - 1, " ");
  return encoder.encode(`${text.slice(0, LINE_BYTES - 1)}\n`);
}

/**
 * Deterministischer Inhalt der Beispieldatei: Textdateien bestehen aus Zeilen fester Länge
 * (64 Bytes), alle anderen aus Signatur plus Pseudo-Bytes. Wahlfreier Zugriff ohne Speicherbedarf,
 * auch für die 20-GB-Datei. Es sind keine echten Bilder oder Medien (nicht dekodierbar).
 */
export function mockFileBytes(name: string, size: number, offset: number, len: number): Uint8Array {
  const end = Math.min(size, offset + len);
  if (offset >= end) return new Uint8Array(0);
  const fixed = SAMPLE_FILES[name] ?? COMPARE_FILES[name];
  if (fixed) return fixed.slice(offset, end);
  const out = new Uint8Array(end - offset);
  if (isTextName(name)) {
    for (let line = Math.floor(offset / LINE_BYTES); line * LINE_BYTES < end; line += 1) {
      const bytes = textLine(line, name);
      const lineStart = line * LINE_BYTES;
      const from = Math.max(offset, lineStart);
      const to = Math.min(end, lineStart + LINE_BYTES);
      out.set(bytes.subarray(from - lineStart, to - lineStart), from - offset);
    }
    return out;
  }
  const magic = MAGIC[extensionOf(name)] ?? [];
  for (let i = 0; i < out.length; i += 1) {
    const position = offset + i;
    out[i] = position < magic.length ? magic[position]! : ((position * 31 + 7) ^ (position >> 8)) & 0xff;
  }
  return out;
}

/** Grobe Typerkennung des Mocks (der Kern erkennt mehr Formate, siehe `detect_kind`). */
function mockKind(head: Uint8Array, name = ""): { kind: FileKind; mime: string } {
  const starts = (bytes: number[], at = 0): boolean => bytes.every((byte, i) => head[at + i] === byte);
  if (starts(MAGIC.png!)) return { kind: "image", mime: "image/png" };
  if (starts(MAGIC.jpg!.slice(0, 3))) return { kind: "image", mime: "image/jpeg" };
  if (starts(MAGIC.gif!)) return { kind: "image", mime: "image/gif" };
  if (starts([...encoder.encode("RIFF")]) && starts([...encoder.encode("WAVE")], 8)) return { kind: "audio", mime: "audio/wav" };
  if (starts([...encoder.encode("ftyp")], 4)) return { kind: "video", mime: "video/mp4" };
  // UTF-16 enthält NUL-Bytes; erst die Marke (BOM) macht es zu Text.
  if (starts([0xff, 0xfe]) || starts([0xfe, 0xff])) return { kind: "text", mime: "text/plain" };
  if (head.includes(0)) return { kind: "binary", mime: "application/octet-stream" };
  if (extensionOf(name) === "svg") return { kind: "image", mime: "image/svg+xml" };
  return { kind: "text", mime: "text/plain" };
}

const defaultScheduler: MockScheduler = {
  setTimeout: (callback, ms) => setTimeout(callback, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

// ---------------------------------------------------------------- Pfade und Namen

// ---------------------------------------------------------------- Archivpfade

const ARCHIVE_PREFIX = "archive:";
const MAX_ARCHIVE_DEPTH = 3;
const MOCK_READ_ONLY = "Archive sind schreibgeschützt";

const isArchivePath = (path: string): boolean => path.startsWith(ARCHIVE_PREFIX);
const encodeOuter = (outer: string): string => outer.replaceAll("%", "%25").replaceAll("!", "%21");
const formatArchive = (outer: string, inner: string): string => `${ARCHIVE_PREFIX}${encodeOuter(outer)}!${inner}`;

/**
 * Zerlegt `archive:<Archivdatei>!<innen>` wie der Kern: nur `%25` und `%21` sind erlaubt, der
 * Innenpfad beginnt mit `/` und enthält weder `..` noch NUL, die Verschachtelung ist auf drei begrenzt.
 */
function splitArchive(path: string, depth = 1): { outer: string; inner: string } {
  const invalid = (): CoreError => new CoreError(`Der Archivpfad ist ungültig: ${path}`, "invalidPath");
  if (depth > MAX_ARCHIVE_DEPTH) throw new CoreError("Archive sind zu tief verschachtelt", "invalidPath");
  const rest = path.slice(ARCHIVE_PREFIX.length);
  const cut = rest.indexOf("!");
  if (cut < 0 || path.includes("\0")) throw invalid();
  const encoded = rest.slice(0, cut);
  const inner = rest.slice(cut + 1);
  if (/%(?!25|21)/.test(encoded) || !inner.startsWith("/") || inner.split("/").includes("..")) throw invalid();
  const outer = encoded.replaceAll("%21", "!").replaceAll("%25", "%");
  if (isArchivePath(outer)) splitArchive(outer, depth + 1); // Tiefe und Form der inneren Ebenen prüfen
  else if (!outer.startsWith("/") || outer.split("/").includes("..")) throw invalid();
  return { outer, inner };
}

/** Löst `.` und `..` auf und liefert einen absoluten Pfad ohne Doppel-Schrägstriche. */
function normalize(path: string): string {
  if (isRemotePath(path)) {
    const ref = splitRemote(path);
    return formatRemote(ref.scheme, ref.id, normalizeLocal(ref.inner));
  }
  if (isArchivePath(path)) {
    const { outer, inner } = splitArchive(path);
    return formatArchive(normalize(outer), normalizeLocal(inner));
  }
  return normalizeLocal(path);
}

function normalizeLocal(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

const basename = (path: string): string => normalize(path).split("/").pop() ?? "";

function parentOf(path: string): string {
  if (isRemotePath(path)) {
    const ref = splitRemote(path);
    return formatRemote(ref.scheme, ref.id, parentOf(ref.inner));
  }
  if (isArchivePath(path)) {
    const { outer, inner } = splitArchive(path);
    return formatArchive(normalize(outer), parentOf(inner));
  }
  const parts = normalize(path).split("/").filter(Boolean);
  parts.pop();
  return `/${parts.join("/")}`;
}

function join(parent: string, name: string): string {
  if (isRemotePath(parent)) {
    const ref = splitRemote(parent);
    return formatRemote(ref.scheme, ref.id, normalizeLocal(`${ref.inner}/${name}`));
  }
  return normalize(`${parent}/${name}`);
}

/** Der Ort, in dem ein Pfad liegt: die Archivdatei oder (lokal) `null`. */
const placeOf = (path: string): string | null => {
  if (isRemotePath(path)) {
    const ref = splitRemote(path);
    return `${ref.scheme}:${ref.id}`;
  }
  return isArchivePath(path) ? splitArchive(path).outer : null;
};
const isArchiveRoot = (path: string): boolean => isArchivePath(path) && splitArchive(path).inner === "/";
/** Wurzel eines Ortes: `/`, die Archivwurzel oder die Wurzel eines Servers. */
const isRootPath = (path: string): boolean =>
  path === "/" || isArchiveRoot(path) || (isRemotePath(path) && splitRemote(path).inner === "/");

// ---------------------------------------------------------------- Serverpfade

/**
 * `ftp:<id>!<pfad>`, `sftp:<id>!<pfad>`, in Einstellungen `conn:<profil-id>!<pfad>`, dazu
 * `plugin:<plugin-id>!<pfad>` (Dateisystem-Plugin); der Pfad steht wörtlich hinter dem ersten `!`.
 */
const isRemotePath = (path: string): boolean => /^(?:ftp|sftp|smb|conn|plugin):/.test(path);
interface RemoteRef {
  scheme: "ftp" | "sftp" | "smb" | "conn" | "plugin";
  id: string;
  inner: string;
}
function splitRemote(path: string): RemoteRef {
  const match = /^(ftp|sftp|smb|conn|plugin):([a-z0-9-]{1,32})!(\/[^]*)$/.exec(path);
  if (!match || path.includes("\0") || match[3]!.split("/").includes("..")) {
    throw new CoreError(`Der Serverpfad ist ungültig: ${path}`, "invalidPath");
  }
  return { scheme: match[1] as RemoteRef["scheme"], id: match[2]!, inner: match[3]! };
}
const formatRemote = (scheme: string, id: string, inner: string): string => `${scheme}:${id}!${inner}`;

/** Archive sind schreibgeschützt: jede Änderung an einem Archivpfad wird abgelehnt. */
function refuseArchiveWrite(...paths: string[]): void {
  if (paths.some(isArchivePath)) throw new CoreError(MOCK_READ_ONLY, "readOnly");
}

/** Namensprüfung wie im Kern: leer, `.`, `..`, Schrägstrich, mehr als 255 Bytes. */
function validateName(name: string): void {
  const tooLong = new TextEncoder().encode(name).length > 255;
  if (name === "" || name === "." || name === ".." || name.includes("/") || name.includes("\0") || tooLong) {
    throw new CoreError(`Ungültiger Name: ${name}`, "invalidName");
  }
}

/** `name.ext` wird zu `name (2).ext`, `.bashrc` zu `.bashrc (2)`. */
function keepBothName(name: string, taken: (candidate: string) => boolean): string {
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : "";
  let counter = 2;
  while (taken(`${stem} (${counter})${ext}`)) counter += 1;
  return `${stem} (${counter})${ext}`;
}

// ---------------------------------------------------------------- Regex (nur Entwurf)

const byteLength = (text: string): number => new TextEncoder().encode(text).length;

/** `$$`, `$1`, `${1}`, `${name}`, `$name` wie in der Rust-Crate `regex` (unbekannte Gruppe = leer). */
function expandReplacement(replacement: string, match: RegExpExecArray): string {
  let out = "";
  for (let i = 0; i < replacement.length; i += 1) {
    const ch = replacement[i]!;
    if (ch !== "$") {
      out += ch;
      continue;
    }
    const rest = replacement.slice(i + 1);
    const braced = /^\{([0-9A-Za-z_]+)\}/.exec(rest);
    const bare = /^([0-9A-Za-z_]+)/.exec(rest);
    if (rest.startsWith("$")) {
      out += "$";
      i += 1;
    } else if (braced || bare) {
      const ref = (braced ?? bare)![1]!;
      const consumed = braced ? braced[0].length : ref.length;
      out += /^[0-9]+$/.test(ref) ? (match[Number(ref)] ?? "") : (match.groups?.[ref] ?? "");
      i += consumed;
    } else out += "$";
  }
  return out;
}

/**
 * NUR ENTWURF (Browser-Mock): wertet mit der JavaScript-Regex aus. Das Produkt nutzt die lineare
 * Rust-Engine; hier können katastrophale Muster hängen, und nur die Syntax-Ablehnung von
 * Rückverweisen/Lookarounds und die Grenzen sind nachgebildet.
 */
function mockRegexReplace(
  names: string[],
  pattern: string,
  replacement: string,
  ignoreCase: boolean,
  all: boolean,
): RegexResult[] {
  const bad = (message: string): CoreError => new CoreError(message, "invalidPattern");
  if (names.length > MAX_REGEX_NAMES) {
    throw new CoreError(`Zu viele Namen (höchstens ${MAX_REGEX_NAMES})`, "invalidInput");
  }
  if (pattern === "") throw bad("Das Suchmuster ist leer");
  if ([...pattern].length > MAX_REGEX_PATTERN_CHARS) {
    throw bad(`Das Suchmuster ist zu lang (höchstens ${MAX_REGEX_PATTERN_CHARS} Zeichen)`);
  }
  if ([...replacement].length > MAX_REGEX_PATTERN_CHARS) {
    throw bad(`Der Ersetzungstext ist zu lang (höchstens ${MAX_REGEX_PATTERN_CHARS} Zeichen)`);
  }
  if (/\(\?<?[=!]|(?<!\\)(?:\\\\)*\\[1-9]|\\k</.test(pattern)) {
    throw bad("Ungültiger regulärer Ausdruck (Rückverweise und Lookarounds gibt es nicht)");
  }
  let regex: RegExp;
  try {
    regex = new RegExp(pattern.replaceAll("(?P<", "(?<"), `gu${ignoreCase ? "i" : ""}`);
  } catch {
    throw bad("Ungültiger regulärer Ausdruck");
  }
  return names.map((name): RegexResult => {
    if (byteLength(name) > MAX_REGEX_NAME_BYTES) {
      return { kind: "error", message: `Der Name ist zu lang (höchstens ${MAX_REGEX_NAME_BYTES} Bytes)` };
    }
    regex.lastIndex = 0;
    let out = "";
    let last = 0;
    for (let match = regex.exec(name); match; match = regex.exec(name)) {
      out += name.slice(last, match.index) + expandReplacement(replacement, match);
      last = match.index + match[0].length;
      if (match[0] === "") regex.lastIndex += 1;
      if (!all) break;
    }
    out += name.slice(last);
    if (byteLength(out) > MAX_REGEX_RESULT_BYTES) {
      return { kind: "error", message: "Das Ergebnis der Ersetzung ist zu lang" };
    }
    return { kind: "ok", name: out };
  });
}

// ---------------------------------------------------------------- Startdaten

function makeEntry(name: string, kind: Entry["kind"], size: number, modified: number, extra: Partial<Entry> = {}): Entry {
  return {
    name,
    kind,
    size,
    modified,
    mode: kind === "dir" ? 0o755 : 0o644,
    hidden: name.startsWith("."),
    linkTarget: null,
    brokenLink: false,
    error: null,
    ...extra,
  };
}

/**
 * Inhalt des Demo-Archivs `Downloads/rezept-paket.tar.gz`: Ordner, Datei, relativer Link und ein
 * verschachteltes Archiv. Eigene Zeitstempel, damit die übrigen Beispieldaten unverändert bleiben.
 */
function demoArchiveTree(now: number): Node {
  const at = now - 86_400_000 * 30;
  const dir = (name: string, children: Node[]): Node => ({
    entry: makeEntry(name, "dir", 0, at),
    children: new Map(children.map((child) => [child.entry.name, child])),
  });
  const file = (name: string, size: number): Node => ({ entry: makeEntry(name, "file", size, at) });
  return dir("", [
    dir("rezepte", [file("pizza.txt", 640), file("brot.txt", 1_280), dir("desserts", [file("tiramisu.txt", 900)])]),
    file("README.md", 300),
    {
      entry: makeEntry("aktuell", "symlink", 16, at, { linkTarget: "rezepte/pizza.txt", mode: 0o777 }),
    },
    {
      entry: makeEntry("extras.zip", "file", 2_000, at),
      archive: dir("", [file("tipps.txt", 500)]),
    },
  ]);
}

/**
 * Demo für Vergleichen/Abgleichen: `links` und `rechts` mit jedem Zustand (siehe Namen), einem nicht
 * lesbaren Teilbaum (`gesperrt`, links) und `text/` für den Textvergleich.
 */
function compareDemoTree(now: number): Node {
  const t0 = now - 86_400_000 * 10;
  const hour = 3_600_000;
  const dir = (name: string, children: Node[] = [], extra: Partial<Entry> = {}): Node => ({
    entry: makeEntry(name, "dir", 0, t0, extra),
    children: new Map(children.map((child) => [child.entry.name, child])),
  });
  const file = (name: string, size: number, modified = t0, version = 0): Node => ({
    entry: makeEntry(name, "file", size, modified),
    ...(version ? { version } : {}),
  });
  const link = (name: string, target: string): Node => ({
    entry: makeEntry(name, "symlink", target.length, t0, { linkTarget: target, mode: 0o777 }),
  });
  const left = dir("links", [
    file("gleich.txt", 1_200),
    file("nur-links.txt", 300),
    file("neuer-links.txt", 500, t0 + hour),
    file("neuer-rechts.txt", 500),
    file("anders.txt", 700),
    file("gleiche-groesse.txt", 400, t0, 1),
    file("typ", 10),
    dir("ordner-nur-links", [file("a.txt", 100), file("b.txt", 200)]),
    dir("gemeinsam", [file("a.txt", 50), file("b.txt", 60, t0 + hour), file("c.txt", 70)]),
    // Nicht lesbar: der ganze Teilbaum ist „unbekannt“, nie „leer“ oder „fehlt“.
    dir("gesperrt", [file("geheim.txt", 10)], { error: "Zugriff verweigert", mode: 0o000 }),
    link("verknuepfung", "gleich.txt"),
    link("verknuepfung-anders", "gleich.txt"),
  ]);
  const right = dir("rechts", [
    file("gleich.txt", 1_200),
    file("nur-rechts.txt", 900),
    file("neuer-links.txt", 500),
    file("neuer-rechts.txt", 500, t0 + hour),
    file("anders.txt", 750, t0 + 1_000),
    file("gleiche-groesse.txt", 400),
    dir("typ", [file("x.txt", 5)]),
    dir("ordner-nur-rechts", [file("r.txt", 80)]),
    dir("gemeinsam", [file("a.txt", 50), file("b.txt", 60), file("d.txt", 90)]),
    dir("gesperrt", [file("daten.txt", 20)]),
    link("verknuepfung", "gleich.txt"),
    link("verknuepfung-anders", "nur-rechts.txt"),
  ]);
  const text = dir("text", [
    file("vergleich-alt.txt", COMPARE_FILES["vergleich-alt.txt"]!.length),
    file("vergleich-neu.txt", COMPARE_FILES["vergleich-neu.txt"]!.length),
    file("vergleich-bin.dat", COMPARE_FILES["vergleich-bin.dat"]!.length),
    file("vergleich-gross.txt", 3 * 1024 * 1024),
  ]);
  return dir("Vergleich", [left, right, text]);
}

function sampleTree(home: string, now: number, samples = false, compareDemo = false): Node {
  let age = 0;
  // Jeder Eintrag bekommt einen anderen, nachvollziehbaren Zeitstempel.
  const stamp = (): number => now - 3_600_000 * (age += 7);
  const dir = (name: string, children: Node[] = [], extra: Partial<Entry> = {}): Node => ({
    entry: makeEntry(name, "dir", 0, stamp(), extra),
    children: new Map(children.map((child) => [child.entry.name, child])),
  });
  const file = (name: string, size: number): Node => ({ entry: makeEntry(name, "file", size, stamp()) });
  const link = (name: string, target: string): Node => ({
    entry: makeEntry(name, "symlink", target.length, stamp(), { linkTarget: target, mode: 0o777 }),
  });
  const demoArchive = (name: string, size: number, at: number): Node => ({
    entry: makeEntry(name, "file", size, stamp()),
    archive: demoArchiveTree(at),
  });

  const userContent = [
    dir("Dokumente", [
      file("notizen.txt", 1_200),
      file("rechnung-oktober.pdf", 210_000),
      file("Änderungen.txt", 4_000),
      file("Übersicht.ods", 38_000),
      file("Bericht 2026-01.txt", 2_900),
      file("Bericht 2026-02.txt", 3_100),
      file("Bericht 2026-10.txt", 3_400),
    ]),
    dir("Bilder", [file("urlaub-2026.jpg", 4_200_000), file("familie.png", 2_200_000), file("Größenvergleich.png", 510_000)]),
    dir("Downloads", [
      file("Datei1.txt", 12),
      file("Datei2.txt", 21),
      file("Datei10.txt", 105),
      file("foto-0412.jpg", 3_400_000),
      file("foto-0413.jpg", 3_100_000),
      file("anleitung.pdf", 980_000),
      demoArchive("rezept-paket.tar.gz", 730_000, now),
      file("setup.AppImage", 129_000_000),
    ]),
    dir("Projekte", [
      dir("dualfile", [dir("src", [file("main.ts", 12_000)]), file("README.md", 2_500)]),
      dir("3mf-katalog", [file("Cargo.toml", 3_000)]),
      // Nicht lesbarer Ordner: Kopieren meldet einen Fehler nur für diesen Eintrag.
      dir("gesperrt", [file("geheim.txt", 10)], { error: "Zugriff verweigert", mode: 0o000 }),
    ]),
    dir("Musik"),
    ...(compareDemo ? [compareDemoTree(now)] : []),
    ...(samples
      ? [
          dir("Ansicht", [
            ...Object.entries(SAMPLE_FILES).map(([name, bytes]) => file(name, bytes.length)),
            ...SAMPLE_BIG.map(([name, size]) => file(name, size)),
          ]),
        ]
      : []),
    dir(".config", [dir("dualfile", [file("settings.json", 128)])]),
    file(".bashrc", 500),
    file(".profile", 220),
    file("große-datei.bin", 20_000_000_000),
    file("start.sh", 800),
    link("Projekte-Link", `${home}/Projekte`),
    link("Defekter-Link", `${home}/gibt-es-nicht`),
  ];

  const parts = home.split("/").filter(Boolean);
  const userName = parts.pop() ?? "demo";
  let userDir = dir(userName, userContent);
  for (const part of parts.reverse()) userDir = dir(part, [userDir]);
  return dir("", [userDir, dir("tmp")]);
}

// ---------------------------------------------------------------- Server (Demo)

/** Feste Demo-Fingerabdrücke (SHA-256, 64 Hex-Zeichen) und das Demo-Passwort. */
export const DEMO_FINGERPRINT_FTPS = "0123456789abcdef".repeat(4);
export const DEMO_FINGERPRINT_SFTP = "fedcba9876543210".repeat(4);
export const DEMO_FINGERPRINT_CHANGED = "a1b2c3d4e5f60718".repeat(4);
export const DEMO_PASSWORD = "demo";
/** Obergrenzen wie im Kern. */
export const MAX_CONNECTIONS = 8;
export const MAX_PROFILES = 100;
const DEMO_HOSTS: Record<string, ConnectionProtocol[]> = { "demo-ftp": ["ftp", "ftps"], "demo-sftp": ["sftp"], "demo-smb": ["smb"] };
const NO_CONTROL = /^[^\u0000-\u001f\u007f-\u009f]*$/;
const disconnected = (): CoreError =>
  new CoreError("Die Verbindung ist getrennt oder nicht mehr vorhanden. Bitte erneut verbinden.", "disconnected");

/** Ordnerbaum eines Demo-Servers (bleibt über Verbindungen hinweg bestehen wie ein echter Server). */
function demoServerTree(host: string, now: number): Node {
  const at = now - 86_400_000 * 3;
  const dir = (name: string, children: Node[] = []): Node => ({
    entry: makeEntry(name, "dir", 0, at),
    children: new Map(children.map((child) => [child.entry.name, child])),
  });
  const file = (name: string, size: number): Node => ({ entry: makeEntry(name, "file", size, at) });
  return host === "demo-ftp"
    ? dir("", [
        dir("pub", [file("readme.txt", 600), file("daten.csv", 12_000), dir("bilder", [file("foto.jpg", 2_400_000)])]),
        dir("upload"),
        file("grosse-datei.bin", 8_000_000),
        file("notizen.txt", 1_500),
      ])
    : dir("", [
        dir("dokumente", [file("bericht.txt", 3_200), file("tabelle.csv", 9_000)]),
        dir("projekte", [dir("app", [file("main.ts", 4_000)]), file("README.md", 800)]),
        file("backup.bin", 6_000_000),
        file("readme.md", 700),
      ]);
}

/** Prüft ein Profil wie der Kern und liefert die bereinigte Fassung (ohne Pins, ohne Kennung). */
function validateProfile(profile: ConnectionProfile): ConnectionProfile {
  if (profile.protocol === "system") {
    let url: URL;
    try { url = new URL(profile.systemUrl ?? ""); } catch { throw new CoreError("Invalid system URL", "invalidInput"); }
    if (!["smb:", "nfs:", "webdav:", "webdavs:", "dav:", "davs:", "afp:"].includes(url.protocol) || url.password || profile.saveSecret || profile.transport === "openssh" || !profile.name.trim() || url.hostname !== profile.host || profile.tls !== "none" || profile.auth.kind !== "password" || profile.allowPlaintext) throw new CoreError("Invalid system profile", "invalidInput");
    return { ...profile, id: "", systemUrl: profile.systemUrl };
  }
  const openssh = profile.transport === "openssh";
  const bad = (what: string): CoreError => new CoreError(`Ungültige Eingabe: ${what}`, "invalidInput");
  const text = (value: unknown): value is string => typeof value === "string";
  if (!text(profile.name) || profile.name.trim() === "" || [...profile.name].length > 100 || !NO_CONTROL.test(profile.name)) {
    throw bad("Name");
  }
  if (openssh && (profile.protocol !== "sftp" || profile.saveSecret)) throw bad("Transport");
  if (profile.transport && !["builtin", "openssh"].includes(profile.transport)) throw bad("Transport");
  if (!["ftp", "ftps", "sftp", "smb", "nfs", "webdav"].includes(profile.protocol)) throw bad("Protokoll");
  if (
    !text(profile.host) ||
    profile.host === "" ||
    profile.host.length > (openssh ? 255 : 253) ||
    !(openssh ? /^[A-Za-z0-9._@:%[\]-]+$/ : /^[A-Za-z0-9.:\-[\]]+$/).test(profile.host) || (openssh && profile.host.startsWith("-"))
  ) {
    throw bad("Server");
  }
  if (!Number.isInteger(profile.port) || profile.port < (openssh ? 0 : 1) || profile.port > 65_535) throw bad("Port");
  if (!text(profile.user) || (!openssh && profile.protocol !== "smb" && profile.protocol !== "nfs" && profile.protocol !== "webdav" && profile.user === "") || profile.user.length > 255 || !NO_CONTROL.test(profile.user)) {
    throw bad("Benutzer");
  }
  if (openssh && profile.user !== "" && (!/^[A-Za-z0-9._@:%[\]-]+$/.test(profile.user) || profile.user.startsWith("-"))) throw bad("Benutzer");
  if (profile.protocol === "smb") {
    const share = profile.share ?? "", domain = profile.domain ?? "";
    if (share.length > 255 || [".", ".."].includes(share) || /[\/\\]/.test(share) || !NO_CONTROL.test(share) || domain.length > 256 || !NO_CONTROL.test(domain) || (!profile.guest && profile.user.toLowerCase() === "guest")) throw bad("SMB");
  }
  if (profile.protocol === "nfs") {
    const exp = profile.export ?? "";
    if (!exp.startsWith("/") || exp.length > 1024 || !NO_CONTROL.test(exp) || exp.includes("\\") || exp.split("/").includes("..")) throw bad("NFS");
  }
  if (profile.protocol === "webdav") {
    // Strict host (no userinfo/port/path/query/fragment, so credentials cannot be redirected) and a
    // base path that is a valid folder; cleartext http needs the explicit confirmation. Mirrors the
    // Rust valid_webdav_host / WebdavProfile::validate.
    const base = profile.share ?? "/";
    if (!/^(\[[0-9a-fA-F:]+\]|[A-Za-z0-9.-]+)$/.test(profile.host)) throw bad("Server");
    if (!base.startsWith("/") || base.length > 1024 || !NO_CONTROL.test(base) || base.split("/").includes("..")) throw bad("WebDAV");
    if (profile.tls === "none" && !profile.allowPlaintext) throw bad("Klartext");
  }
  const folder = profile.startFolder;
  if (
    folder !== null &&
    (!text(folder) || !folder.startsWith("/") || folder.length > 1024 || !NO_CONTROL.test(folder) || folder.split("/").includes(".."))
  ) {
    throw bad("Startordner");
  }
  const tlsOk =
    profile.protocol === "ftps" ? profile.tls === "explicit" || profile.tls === "implicit"
    : profile.protocol === "webdav" ? profile.tls === "explicit" || profile.tls === "none"
    : profile.tls === "none";
  if (!tlsOk) throw bad("Verschlüsselung");
  const auth = profile.auth;
  if (auth?.kind === "keyFile") {
    if (profile.protocol !== "sftp" || !text(auth.path) || auth.path === "" || auth.path.length > 4096 || auth.path.includes("\0")) {
      throw bad("Schlüsseldatei");
    }
  } else if (auth?.kind !== "password") throw bad("Anmeldeart");
  return {
    id: "",
    name: profile.name,
    protocol: profile.protocol,
    transport: profile.transport ?? "builtin",
    share: profile.share ?? "", domain: profile.domain ?? "", guest: profile.guest ?? false,
    export: profile.export ?? "", uid: profile.uid ?? 1000, gid: profile.gid ?? 1000,
    host: profile.host,
    port: profile.port,
    user: profile.user,
    startFolder: folder,
    tls: profile.tls,
    allowPlaintext: profile.protocol === "ftp" && profile.allowPlaintext === true,
    auth: auth.kind === "keyFile" ? { kind: "keyFile", path: auth.path } : { kind: "password" },
    saveSecret: profile.saveSecret === true,
  };
}

// ---------------------------------------------------------------- Aufträge

interface Job {
  paused: boolean;
  gate: (() => void) | null;
  undoPairs?: [string, string][];
  undoReplaced?: boolean;
  id: number;
  cancelled: boolean;
  /** Antwort „für alle“, gilt für den Rest des Auftrags. */
  policy: Resolution | null;
  timer: unknown;
  wake: (() => void) | null;
  conflict: ((answer: ConflictAnswer) => void) | null;
  /** Läuft gerade eine Datei-Übertragung mit Server: legt bei Abbruch die fortsetzbare Teildatei an. */
  inFlight: { commit: () => { path: string; message: string } | null } | null;
}

class CancelledSignal extends Error {}

/** Ergebnis eines Ordnervergleichs (Spiegel von `CompareResult` im Kern). */
interface MockCompare {
  id: number;
  left: string;
  right: string;
  mode: CompareMode;
  recursive: boolean;
  entries: CompareEntry[];
  incomplete: boolean;
  cancelled: boolean;
  issues: string[];
}

/** Zustand eines Zieleintrags beim Planen (Spiegel von `TargetVersion`). */
interface TargetVersion {
  node: Node;
  kind: EntryKind;
  size: number;
  modified: number | null;
  version: number;
  linkTarget: string | null;
  children: string[];
}

interface PlannedAction {
  kind: SyncActionKind;
  /** Pfad relativ zum Zielordner. */
  rel: string;
  entryKind: EntryKind;
  size: number;
  from: string | null;
  to: string;
  /** Überschreiben und Löschen: Stand des Ziels beim Planen. */
  expected: TargetVersion | null;
}

interface MockSyncPlan {
  token: string;
  compareId: number;
  mode: SyncMode;
  source: string;
  target: string;
  actions: PlannedAction[];
  notes: SyncPlanNote[];
  incomplete: boolean;
}

const isTerminal = (state: JobStatus["state"]): boolean =>
  state === "done" || state === "failed" || state === "cancelled";

function countByKind(nodes: Node[]): { files: number; dirs: number; links: number; bytes: number } {
  const totals = { files: 0, dirs: 0, links: 0, bytes: 0 };
  const visit = (node: Node): void => {
    if (node.entry.kind === "file") {
      totals.files += 1;
      totals.bytes += node.entry.size;
    } else if (node.entry.kind === "symlink") totals.links += 1;
    else totals.dirs += 1;
    node.children?.forEach(visit);
  };
  nodes.forEach(visit);
  return totals;
}

function cloneNode(node: Node, name: string): Node {
  const copy: Node = { entry: { ...node.entry, name, hidden: name.startsWith(".") } };
  if (node.children) {
    copy.children = new Map([...node.children].map(([key, child]) => [key, cloneNode(child, child.entry.name)]));
  }
  if (node.archive) copy.archive = cloneNode(node.archive, "");
  if (node.version) copy.version = node.version;
  return copy;
}

const describe = (verb: string, paths: string[]): string =>
  `${verb}: ${basename(paths[0] ?? "")}${paths.length > 1 ? ` (+${paths.length - 1})` : ""}`;
const titleOf = (kind: string, paths: string[]): JobTitle => ({ kind, first: basename(paths[0] ?? ""), more: Math.max(0, paths.length - 1) });

/** Beispielinhalt der Logdatei für Demo und Tests; die Platzhalter ersetzt `logRead`. */
const MOCK_LOG = [
  "2026-11-16 14:58:02 INFO  Dualfile 0.3.0 gestartet · Linux x86_64 · KDE Plasma 6.2 (Wayland) · AppImage",
  "2026-11-16 14:58:02 INFO  Einstellungen geladen aus {HOME}/.config/dualfile/settings.json",
  "2026-11-16 14:59:40 WARN  Ordner nicht lesbar: {HOME}/Projekte/.cache (Zugriff verweigert)",
  "2026-11-16 15:01:12 INFO  Verbindung SMB {SRV}/daten als {USR} aufgebaut",
  "2026-11-16 15:01:31 ERROR Kopieren {HOME}/Dokumente/bericht.pdf → smb://{SRV}/daten/bericht.pdf fehlgeschlagen",
  "                            Ursache: Verbindung vom Server getrennt (os error 104) nach 4,2 MB von 6,8 MB",
  "2026-11-16 15:01:31 INFO  Teildatei auf dem Ziel gelöscht, Quelle unverändert",
  "2026-11-16 15:02:05 WARN  Update-Prüfung: keine Antwort von GitHub (Zeitüberschreitung 10 s)",
].join("\n");

export function createMockApi(options: MockOptions = {}): MockApi {
  const unmountedSystemPlaces = new Set<string>();
  const home = normalize(options.home ?? "/home/demo");
  const scheduler = options.scheduler ?? defaultScheduler;
  const clock = options.now ?? (() => Date.now());
  const root = sampleTree(home, clock(), options.samples ?? false, options.compareDemo ?? options.samples ?? false);

  const handlers = new Set<(event: CoreEvent) => void>();
  const statuses = new Map<number, JobStatus>();
  const jobs = new Map<number, Job>();
  const trashed = new Map<string, { originalPath: string; node: Node }>();
  const deletePlans = new Map<string, { paths: string[]; connectionId: string | null; profileId: string | null }>();
  let settings: Settings | null = options.initialSettings ? structuredClone(options.initialSettings) : null;
  let nextJobId = 1;
  let nextPlan = 1;

  // ---------------------------------------------------------------- Server-Verbindungen

  interface ProfileRecord {
    profile: ConnectionProfile;
    pins: PinInfo[];
  }
  interface LiveConnection {
    profileId: string;
    scheme: "ftp" | "sftp" | "smb";
    tree: Node;
    root: string;
  }
  interface PendingTrust {
    fingerprint: string;
    secret: string | undefined;
  }
  const secretKind: SecretStoreKind = options.keychain === false ? "session" : "keychain";
  const profiles = new Map<string, ProfileRecord>();
  const secrets = new Map<string, { value: string; kind: SecretStoreKind }>();
  const live = new Map<string, LiveConnection>();
  const byProfile = new Map<string, string>();
  const pending = new Map<string, PendingTrust>();
  const servers = new Map<string, Node>();
  /** Fortsetzbare Teildateien je Zielordner: Name der Zieldatei → Quellidentität und geschriebene Bytes. */
  const partials = new WeakMap<Node, Map<string, { identity: string; written: number }>>();
  let nextProfile = 1;
  let nextConnection = 1;

  const plugins = createMockPlugins(clock, options.plugins);

  /** Baum eines Plugin-Ortes; ein nicht aktives oder unbekanntes Plugin ist „getrennt“. */
  const pluginTree = (id: string): Node => {
    const tree = plugins.tree(id);
    if (!tree) throw disconnected();
    return tree as Node;
  };
  /** Schreiben auf einen Plugin-Ort: nur mit Erlaubnis des Nutzers (`writeGranted`), sonst `readOnly`. */
  const refusePluginWrite = (...paths: string[]): void => {
    for (const path of paths) {
      if (!path.startsWith("plugin:")) continue;
      const access = plugins.writeAccess(splitRemote(path).id);
      if (access === "inactive") throw disconnected();
      if (access === "readOnly") throw new CoreError("Das Plugin darf hier nicht schreiben", "readOnly");
    }
  };

  const liveConnection = (id: string, scheme?: string): LiveConnection => {
    const connection = live.get(id);
    if (!connection) throw disconnected();
    if (scheme !== undefined && scheme !== connection.scheme) {
      throw new CoreError("Der Serverpfad gehört nicht zu dieser Verbindung", "invalidPath");
    }
    return connection;
  };
  const record = (profileId: string): ProfileRecord => {
    const found = profiles.get(profileId);
    if (!found) throw new CoreError("Ungültige Eingabe: Das Verbindungsprofil gibt es nicht", "invalidInput");
    return found;
  };
  const closeConnection = (profileId: string): void => {
    const connectionId = byProfile.get(profileId);
    if (connectionId !== undefined) live.delete(connectionId);
    byProfile.delete(profileId);
  };
  const entryOf = (rec: ProfileRecord): ConnectionEntry => {
    const connectionId = byProfile.get(rec.profile.id) ?? null;
    const connection = connectionId === null ? undefined : live.get(connectionId);
    const mounted = rec.profile.protocol === "system" && !unmountedSystemPlaces.has(rec.profile.systemUrl ?? "")
      ? mockSystemPlaces.find(p => p.url === rec.profile.systemUrl)?.mountedAt : null;
    const secret = secrets.get(rec.profile.id);
    return {
      profile: structuredClone(rec.profile),
      pins: structuredClone(rec.pins),
      connected: !!mounted || connection !== undefined,
      connectionId: connection ? connectionId : null,
      root: mounted ?? connection?.root ?? null,
      secretSet: secret !== undefined,
      secretStore: secret?.kind ?? "none",
    };
  };
  const pinFor = (rec: ProfileRecord): PinInfo | undefined =>
    rec.pins.find(
      (pin) =>
        pin.protocol === rec.profile.protocol &&
        pin.host.toLowerCase() === rec.profile.host.toLowerCase() &&
        pin.port === rec.profile.port,
    );

  /** Zweiter Teil des Verbindens (nach der Vertrauensfrage): Anmeldung prüfen, Verbindung aufbauen. */
  const finishConnect = (rec: ProfileRecord, secret: string | undefined): ConnectOutcome => {
    const { profile } = rec;
    const effective = profile.transport === "openssh" ? "" : secret ?? (profile.saveSecret ? secrets.get(profile.id)?.value : undefined);
    const authOk = (profile.protocol === "smb" && profile.guest === true) || profile.transport === "openssh" || profile.auth.kind === "keyFile" || effective === DEMO_PASSWORD;
    if (profile.protocol === "smb" && !profile.guest && !profile.user) throw new CoreError("SMB sign-in requires a user", "smbAuth");
    if (!authOk) throw new CoreError("Die Anmeldung am Server ist fehlgeschlagen.", profile.protocol === "smb" ? "smbAuth" : "auth");
    if (live.size >= MAX_CONNECTIONS) {
      throw new CoreError(`Es sind schon ${MAX_CONNECTIONS} Verbindungen offen; bitte erst eine trennen`, "tooManyConnections");
    }
    const hostKey = profile.host.toLowerCase() + (profile.protocol === "smb" ? ":smb" : "");
    let tree = servers.get(hostKey);
    if (!tree) {
      tree = demoServerTree(hostKey, clock());
      if (profile.protocol === "smb") tree = { entry: makeEntry("", "dir", 0, clock()), children: new Map(["team", "public"].map(name => [name, { ...demoServerTree(hostKey, clock()), entry: makeEntry(name, "dir", 0, clock()) }])) };
      servers.set(hostKey, tree);
    }
    if (profile.protocol === "smb" && profile.share) {
      const share = tree.children?.get(profile.share);
      if (!share) throw new CoreError("SMB share missing", "smbShareMissing");
      tree = share;
    }
    const connectionId = `c${nextConnection++}`;
    const scheme = profile.protocol === "smb" ? "smb" : profile.protocol === "sftp" ? "sftp" : "ftp";
    const root = formatRemote(scheme, connectionId, profile.startFolder ?? "/");
    live.set(connectionId, { profileId: profile.id, scheme, tree, root });
    byProfile.set(profile.id, connectionId);
    try {
      needDir(root);
    } catch (error) {
      closeConnection(profile.id);
      throw error;
    }
    // Nur das Geheimnis, das der Nutzer zum Merken freigegeben hat, wird behalten.
    if (profile.saveSecret && secret !== undefined && secret !== "") secrets.set(profile.id, { value: secret, kind: secretKind });
    return { state: "connected", connectionId, root, secretStore: secrets.get(profile.id)?.kind ?? "none" };
  };

  const emit = (event: CoreEvent): void => handlers.forEach((handler) => handler(event));
  const resolvePath = (path: string): string => {
    if (path.startsWith("conn:")) {
      // Profilpfad aus den Einstellungen: nur mit verbundenem Profil, sonst „getrennt“.
      const ref = splitRemote(path);
      const connectionId = byProfile.get(ref.id);
      const connection = connectionId === undefined ? undefined : live.get(connectionId);
      if (connectionId === undefined || !connection) throw disconnected();
      return normalize(formatRemote(connection.scheme, connectionId, ref.inner));
    }
    if (isRemotePath(path) || isArchivePath(path)) return normalize(path);
    if (path === "~" || path.startsWith("~/")) return normalize(`${home}${path.slice(1)}`);
    if (!path.startsWith("/")) throw new CoreError(`Kein absoluter Pfad: ${path}`, "invalidPath");
    return normalize(path);
  };
  const find = (path: string): Node | null => {
    let node = root;
    let walk = normalize(path);
    if (isRemotePath(walk)) {
      const ref = splitRemote(walk);
      node = ref.scheme === "plugin" ? pluginTree(ref.id) : liveConnection(ref.id, ref.scheme).tree;
      walk = ref.inner;
    } else if (isArchivePath(walk)) {
      const { outer, inner } = splitArchive(walk);
      const file = find(outer);
      if (!file) return null;
      if (file.children) throw new CoreError("Der Pfad ist keine Archivdatei", "invalidPath");
      if (!file.archive) throw new CoreError("Die Datei ist kein erkanntes Archiv", "archive");
      node = file.archive;
      walk = inner;
    }
    for (const part of walk.split("/").filter(Boolean)) {
      const child = node.children?.get(part);
      if (!child) return null;
      node = child;
    }
    return node;
  };
  const need = (path: string): Node => {
    const node = find(path);
    if (!node) throw new CoreError(`Nicht gefunden: ${path}`, "notFound");
    return node;
  };
  const needDir = (path: string): Node => {
    const node = need(path);
    if (!node.children) throw new CoreError(`Kein Ordner: ${path}`, "notDirectory");
    return node;
  };
  const changed = (path: string): void => emit({ type: "dirChanged", path: normalize(path) });

  // Links sind defekt, wenn ihr Ziel (relativ zum Ordner des Links) nicht existiert.
  const snapshot = (node: Node, parentPath: string): Entry => {
    const entry = { ...node.entry };
    if (entry.kind === "symlink" && entry.linkTarget !== null) {
      const target = entry.linkTarget.startsWith("/") ? entry.linkTarget : `${parentPath}/${entry.linkTarget}`;
      entry.brokenLink = find(target) === null;
    }
    return entry;
  };

  const laneIds = new Map<string, string>();
  const destinationLane = (path: string): string => {
    const key = isRemotePath(path) ? placeOf(path)! : path.startsWith("/media/") || path.startsWith("/mnt/") ? path.split("/").slice(0, 3).join("/") : "local";
    if (!laneIds.has(key)) laneIds.set(key, `lane-${laneIds.size + 1}`);
    return laneIds.get(key)!;
  };
  const newJob = (name: string, bytesTotal: number, lane: string | null = null, title?: JobTitle): Job => {
    const id = nextJobId++;
    const job: Job = { id, paused: false, gate: null, cancelled: false, policy: null, timer: null, wake: null, conflict: null, inFlight: null };
    jobs.set(id, job);
    const initial: JobStatus = {
      id,
      name,
      ...(title ? { title } : {}),
      state: "queued",
      filesDone: 0,
      bytesDone: 0,
      bytesTotal,
      etaSeconds: null, queuePosition: null, lane,
      bytesPerSecond: 0,
      failed: [],
      skipped: [],
      warnings: [],
    };
    statuses.set(id, initial);
    emit({ type: "job", status: structuredClone(initial) });
    return job;
  };

  // Ein beendeter Auftrag ändert sich nicht mehr (z. B. nach Abbruch).
  const publish = (id: number, update: Partial<JobStatus>): void => {
    const old = statuses.get(id);
    if (!old || isTerminal(old.state)) return;
    const next = { ...old, ...update };
    next.etaSeconds = next.state === "running" && next.bytesPerSecond > 0 ? Math.ceil(Math.max(0, next.bytesTotal - next.bytesDone) / next.bytesPerSecond) : null;
    if (next.state !== "queued" && next.state !== "paused") next.queuePosition = null;
    statuses.set(id, next);
    emit({ type: "job", status: structuredClone(next) });
    if (isTerminal(next.state)) {
      for (const pending of jobs.values()) pending.gate?.();
    }
  };
  const statusOf = (id: number): JobStatus => {
    const current = statuses.get(id);
    if (!current) throw new CoreError(`Auftrag ${id} nicht gefunden`, "notFound");
    return current;
  };
  const addFailure = (id: number, path: string, message: string, kind?: string): void =>
    publish(id, { failed: [...statusOf(id).failed, { path, message, ...(kind ? { kind } : {}) }] });

  /** Paused jobs retain their destination lane while freeing the concurrency slot. */
  const checkpoint = async (job: Job): Promise<void> => {
    while (!job.cancelled) {
      const status = statusOf(job.id);
      const earlier = [...jobs.values()].filter((other) => other.id < job.id && !isTerminal(statusOf(other.id).state) && status.lane !== null && statusOf(other.id).lane === status.lane);
      const active = [...jobs.values()].filter((other) => other !== job && !other.paused && statusOf(other.id).state === "running");
      if (!job.paused && earlier.length === 0 && active.length < 2) {
        publish(job.id, { state: "running", queuePosition: null });
        return;
      }
      publish(job.id, { state: job.paused ? "paused" : "queued", queuePosition: earlier.length + 1, bytesPerSecond: 0 });
      await new Promise<void>((resolve) => { job.gate = resolve; });
      job.gate = null;
    }
    throw new CancelledSignal();
  };
  /** Wartet einen Schritt des Zeitgebers ab; ein Abbruch weckt sofort. */
  const pause = (job: Job): Promise<void> =>
    new Promise<void>((resolve) => {
      job.wake = resolve;
      job.timer = scheduler.setTimeout(() => {
        job.timer = null;
        job.wake = null;
        resolve();
      }, MOCK_STEP_MS);
    }).then(() => {
      if (job.cancelled) throw new CancelledSignal();
      return checkpoint(job);
    });

  const askConflict = async (
    job: Job,
    sourcePath: string,
    destinationPath: string,
    source: Node,
    destination: Node,
  ): Promise<ConflictAnswer> => {
    if (job.policy) return { resolution: job.policy, applyToAll: true };
    const answer = await new Promise<ConflictAnswer>((resolve) => {
      job.conflict = resolve;
      emit({
        type: "conflict",
        conflict: {
          jobId: job.id,
          sourcePath,
          destinationPath,
          source: { ...source.entry },
          destination: { ...destination.entry },
        },
      });
    });
    job.conflict = null;
    if (job.cancelled) throw new CancelledSignal();
    if (answer.resolution !== "cancel") await checkpoint(job);
    if (answer.applyToAll && answer.resolution !== "cancel") job.policy = answer.resolution;
    return answer;
  };

  /**
   * Überträgt einen Eintrag samt Inhalt. Gibt zurück, ob die Quelle vollständig übertragen
   * wurde (nur dann darf beim Verschieben die Quelle verschwinden).
   */
  const pendingErrors = new Map<number, (decision: import("./types").ItemErrorDecision) => void>();
  const simulatedErrors = new Set<string>();
  const transfer = async (
    job: Job,
    moving: boolean,
    touched: Set<string>,
    sourcePath: string,
    source: Node,
    sourceParent: Node,
    targetPath: string,
    target: Node,
  ): Promise<boolean> => {
    while (true) {
      try {
        const key = `${job.id}:${sourcePath}`;
        if (options.itemError && !source.children && !simulatedErrors.has(key)) {
          simulatedErrors.add(key);
          throw new CoreError("Die Datei konnte nicht verarbeitet werden", "io");
        }
        return await transferAttempt(job, moving, touched, sourcePath, source, sourceParent, targetPath, target);
      } catch (error) {
        if (error instanceof CancelledSignal) throw error;
        if (!options.itemError) throw error;
        const kind = error instanceof CoreError && error.kind === "verifyFailed" ? "verifyFailed" : "io";
        const decision = await new Promise<import("./types").ItemErrorDecision>(resolve => {
          if (job.cancelled) { resolve("abort"); return; }
          pendingErrors.set(job.id, resolve);
          emit({ type: "itemError", error: { jobId: job.id, name: source.entry.name, kind, message: kind === "verifyFailed" ? "Die Prüfsumme stimmt nach dem Übertragen nicht überein" : "Die Datei konnte nicht verarbeitet werden" } });
        });
        if (decision === "abort" || job.cancelled) throw new CancelledSignal();
        if (decision === "retry") continue;
        addFailure(job.id, sourcePath, error instanceof Error ? error.message : "", kind);
        return false;
      }
    }
  };
  const transferAttempt = async (
    job: Job,
    moving: boolean,
    touched: Set<string>,
    sourcePath: string,
    source: Node,
    sourceParent: Node,
    targetPath: string,
    target: Node,
  ): Promise<boolean> => {
    const name = source.entry.name;
    if (source.entry.error) {
      addFailure(job.id, sourcePath, source.entry.error);
      return false;
    }
    if (join(targetPath, name) === normalize(sourcePath)) {
      addFailure(job.id, sourcePath, "Quelle und Ziel sind dieselbe Datei");
      return false;
    }

    let finalName = name;
    const existing = target.children?.get(name);
    let merge = false;
    if (existing) {
      if (source.children && existing.children) merge = true; // Ordner werden zusammengeführt
      else {
        const answer = await askConflict(job, sourcePath, join(targetPath, name), source, existing);
        if (answer.resolution === "cancel") throw new CancelledSignal();
        if (answer.resolution === "skip") {
          publish(job.id, { skipped: [...statusOf(job.id).skipped, sourcePath] });
          return false;
        }
        if (answer.resolution === "keepBoth") {
          finalName = keepBothName(name, (candidate) => target.children?.has(candidate) ?? false);
        } else { job.undoReplaced = true; target.children?.delete(name); } // ausdrücklich bestätigt
      }
    }

    touched.add(targetPath);
    if (source.children) {
      let node = target.children?.get(finalName);
      if (!merge || !node) {
        node = { entry: { ...source.entry, name: finalName, hidden: finalName.startsWith(".") }, children: new Map() };
        target.children?.set(finalName, node);
      }
      let complete = true;
      for (const child of [...source.children.values()]) {
        const ok = await transfer(
          job,
          moving,
          touched,
          join(sourcePath, child.entry.name),
          child,
          source,
          join(targetPath, finalName),
          node,
        );
        complete = complete && ok;
      }
      if (moving && complete) {
        sourceParent.children?.delete(name);
        touched.add(parentOf(sourcePath));
      }
      if (!merge && (!moving || complete)) job.undoPairs?.push([sourcePath, join(targetPath, finalName)]);
      return complete;
    }

    // Datei oder Link: Größe blockweise „übertragen“, damit der Fortschritt sichtbar wächst.
    const size = source.entry.size;
    const chunks = Math.max(1, Math.min(MAX_CHUNKS, Math.ceil(size / CHUNK_BYTES)));
    const remote = isRemotePath(sourcePath) || isRemotePath(targetPath);
    const partName = `${finalName}.dualfile-part`;
    const identity = `${size}:${source.entry.modified}:${source.version ?? 0}`;
    let done = 0;
    if (remote) {
      const part = target.children?.get(partName);
      const meta = partials.get(target)?.get(finalName);
      if (part && !meta) {
        addFailure(job.id, join(targetPath, partName), "Die Hilfsdatei stammt nicht von Dualfile und wird nicht überschrieben");
        return false;
      }
      if (part && meta && meta.identity === identity && meta.written === part.entry.size && meta.written <= size) {
        done = meta.written; // Fortsetzen ab dem Stand der Teildatei
        publish(job.id, {
          bytesDone: statusOf(job.id).bytesDone + done,
          warnings: [
            ...statusOf(job.id).warnings,
            { path: sourcePath, message: `Fortgesetzt: ${name} ab ${done} von ${size} Bytes` },
          ],
        });
      } else if (part && meta) {
        // Die Quelle hat sich geändert: von vorn beginnen.
        target.children?.delete(partName);
        partials.get(target)?.delete(finalName);
      }
    }
    let written = done;
    const checkLive = (): void => {
      for (const path of [sourcePath, targetPath]) {
        if (!isRemotePath(path)) continue;
        const ref = splitRemote(path);
        if (ref.scheme === "plugin") pluginTree(ref.id);
        else liveConnection(ref.id);
      }
    };
    const commit = (): { path: string; message: string } | null => {
      if (!remote || written <= 0 || written >= size) return null;
      target.children?.set(partName, { entry: makeEntry(partName, "file", written, clock()) });
      const table = partials.get(target) ?? new Map();
      table.set(finalName, { identity, written });
      partials.set(target, table);
      return {
        path: sourcePath,
        message: `Übertragung unterbrochen: ${name}. Die Teildatei „${partName}“ bleibt erhalten; beim nächsten Kopieren derselben Datei wird die Übertragung fortgesetzt.`,
      };
    };
    job.inFlight = remote ? { commit } : null;
    try {
      for (let index = 0; index < chunks; index += 1) {
        const end = index === chunks - 1 ? size : Math.floor(size / chunks) * (index + 1);
        if (done > 0 && end <= done) continue;
        await pause(job);
        if (remote) checkLive();
        const part = end - done;
        done = end;
        written = end;
        publish(job.id, {
          state: "running",
          bytesDone: statusOf(job.id).bytesDone + part,
          bytesPerSecond: BYTES_PER_SECOND,
        });
      }
    } catch (error) {
      if (!(error instanceof CancelledSignal)) {
        const warning = commit();
        if (warning) publish(job.id, { warnings: [...statusOf(job.id).warnings, warning] });
      }
      throw error;
    } finally {
      job.inFlight = null;
    }
    if (remote) {
      target.children?.delete(partName);
      partials.get(target)?.delete(finalName);
    }
    target.children?.set(finalName, cloneNode(source, finalName));
    const policy = settings?.verifyPolicy ?? "auto";
    if (options.verifyFailure && (policy === "always" || (policy === "auto" && (isRemotePath(targetPath) || targetPath.startsWith("/media/"))))) {
      throw new CoreError("Die Prüfsumme stimmt nach dem Übertragen nicht überein", "verifyFailed");
    }
    publish(job.id, { filesDone: statusOf(job.id).filesDone + 1 });
    if (moving) {
      sourceParent.children?.delete(name);
      touched.add(parentOf(sourcePath));
    }
    job.undoPairs?.push([sourcePath, join(targetPath, finalName)]);
    return true;
  };

  const undoHandlers = new Set<() => void>();
  const undoChanged = (): void => undoHandlers.forEach((h) => h());
  const localUndoPath = (path: string): boolean => path.startsWith("/") && !isRemotePath(path) && !isArchivePath(path);
  type UndoItem = { original: string; target: string; stamp: string; done: boolean };
  const history: { entry: UndoEntry; items: UndoItem[] }[] = [];
  let nextUndo = 1;
  const stamp = (node: Node): string => node.children ? "dir" : JSON.stringify([node.entry.kind, node.entry.size, node.entry.modified, node.version ?? 0]);
  const recordUndo = (kind: UndoKind, pairs: [string, string][]): void => {
    if (!pairs.length || pairs.some(([a, b]) => !localUndoPath(a) || (kind !== "trash" && !localUndoPath(b)))) return;
    const items: UndoItem[] = [];
    for (const [original, target] of pairs) {
      const node = kind === "trash" ? trashed.get(target)?.node : find(target);
      if (!node) return;
      items.push({ original, target, stamp: stamp(node), done: false });
    }
    history.unshift({ entry: { id: nextUndo++, time: clock(), kind, first: pairs[0]![0].split("/").pop()!, more: pairs.length - 1, label: `undoLabel.${kind}`, undone: false }, items });
    history.splice(200);
    undoChanged();
  };
  const runUndo = async (id: number): Promise<UndoResult> => {
    const job = newJob("Rückgängig", 0);
    const result: UndoResult = { jobId: job.id, ok: false, done: 0, remaining: 0, cancelled: false, errors: [] };
    const record = history.find((r) => r.entry.id === id);
    if (!record) result.errors.push("notFound");
    else if (record.entry.undone) result.errors.push("alreadyUndone");
    else {
      try {
        for (const item of record.items.filter((i) => !i.done)) {
          await pause(job);
          const kind = record.entry.kind;
          const node = kind === "trash" ? trashed.get(item.target)?.node : find(item.target);
          let error: UndoErrorKind | undefined;
          if (!node || stamp(node) !== item.stamp) error = "changed";
          else if (kind === "mkdir" && node.children?.size) error = "changed";
          else if (["move", "rename", "trash"].includes(kind) && find(item.original)) error = "conflict";
          else if (["move", "rename", "trash"].includes(kind) && !find(parentOf(item.original))?.children) error = "io";
          if (error) { result.errors.push(error); continue; }
          if (kind === "copy" || kind === "mkdir") {
            trashed.set(`trash://undo-${id}/${result.done}`, { originalPath: item.target, node: node! });
            need(parentOf(item.target)).children!.delete(basename(item.target));
          } else {
            if (kind === "trash") trashed.delete(item.target);
            else need(parentOf(item.target)).children!.delete(basename(item.target));
            node!.entry = { ...node!.entry, name: basename(item.original), hidden: basename(item.original).startsWith(".") };
            need(parentOf(item.original)).children!.set(basename(item.original), node!);
            changed(parentOf(item.original));
          }
          if (kind !== "trash") changed(parentOf(item.target));
          item.done = true;
          result.done++;
        }
      } catch (error) {
        if (error instanceof CancelledSignal) result.cancelled = true;
        else result.errors.push("io");
      }
      result.remaining = record.items.filter((i) => !i.done).length;
      record.entry.undone = result.remaining === 0;
      result.ok = record.entry.undone && !result.cancelled;
      if (result.done) undoChanged();
    }
    publish(job.id, { state: result.cancelled ? "cancelled" : result.errors.length ? "failed" : "done", filesDone: result.done,
      failed: result.errors.map((message) => ({ path: "", message })) });
    return result;
  };

  const startTransfer = (sources: string[], destination: string, moving: boolean): number => {
    const targetPath = resolvePath(destination);
    const paths = sources.map(resolvePath);
    // In ein Archiv schreiben und aus einem Archiv verschieben (= Quelle löschen) geht nie.
    refuseArchiveWrite(targetPath);
    refusePluginWrite(targetPath);
    if (moving) {
      refuseArchiveWrite(...paths);
      refusePluginWrite(...paths);
    }
    const target = needDir(targetPath);
    const nodes = paths.map(need);
    if (paths.some((path) => placeOf(path) !== placeOf(paths[0]!))) {
      throw new CoreError("Die Einträge liegen nicht im selben Ordner oder Archiv", "invalidInput");
    }
    for (const path of paths) {
      if (isRootPath(path)) throw new CoreError("Die Wurzel kann nicht übertragen werden", "invalidPath");
      if (targetPath === path || targetPath.startsWith(`${path}/`)) {
        throw new CoreError(`Das Ziel liegt innerhalb der Quelle: ${path}`, "invalidTarget");
      }
    }
    if (isRemotePath(targetPath) && paths.some((path) => isRemotePath(path) && placeOf(path) !== placeOf(targetPath))) {
      throw new CoreError("Kopieren und Verschieben zwischen zwei Servern wird nicht unterstützt", "unsupported");
    }
    const verb = moving ? "Verschieben" : isArchivePath(paths[0] ?? "") ? "Entpacken" : "Kopieren";
    const kind = moving ? "move" : isArchivePath(paths[0] ?? "") ? "extract" : "copy";
    const job = newJob(describe(verb, paths), countByKind(nodes).bytes, destinationLane(targetPath), titleOf(kind, paths));
    job.undoPairs = [];

    const run = async (): Promise<void> => {
      const touched = new Set<string>();
      try {
        for (const path of paths) {
          await pause(job); // auch der erste Schritt läuft über den Zeitgeber
          const node = find(path);
          const parent = find(parentOf(path));
          if (!node || !parent) {
            addFailure(job.id, path, "Nicht mehr vorhanden");
            continue;
          }
          await transfer(job, moving, touched, path, node, parent, targetPath, target);
          touched.forEach(changed);
          touched.clear();
        }
        const failed = statusOf(job.id).failed.length > 0;
        publish(job.id, { state: failed ? "failed" : "done", bytesPerSecond: 0, verified: !failed && statusOf(job.id).filesDone > 0 && statusOf(job.id).skipped.length === 0 && (settings?.verifyPolicy === "always" || ((settings?.verifyPolicy ?? "auto") === "auto" && (isRemotePath(targetPath) || targetPath.startsWith("/media/")))) });
      } catch (error) {
        if (error instanceof CancelledSignal) publish(job.id, { state: "cancelled", bytesPerSecond: 0 });
        else {
          const message = error instanceof Error ? error.message : "Unbekannter Fehler";
          addFailure(job.id, paths[0] ?? "", message, error instanceof CoreError ? error.kind : undefined);
          publish(job.id, { state: "failed", bytesPerSecond: 0 });
        }
      } finally {
        if (!job.undoReplaced && [...paths, targetPath].every(localUndoPath)) {
          recordUndo(moving ? "move" : "copy", (job.undoPairs ?? []).filter(([, b], _, pairs) => !pairs.some(([, p]) => p !== b && b.startsWith(`${p}/`))));
        }
        touched.forEach(changed);
      }
    };
    void run();
    return job.id;
  };

  // Datei hinter einem Pfad für den Lister: folgt Links, nur reguläre Dateien (wie der Kern).
  const regularFile = (path: string): { node: Node; name: string } => {
    let current = resolvePath(path);
    for (let hops = 0; hops < 8; hops += 1) {
      const node = need(current);
      if (node.children) throw new CoreError(`Der Pfad ist ein Ordner: ${current}`, "io");
      if (node.entry.kind !== "symlink") return { node, name: node.entry.name };
      if (isArchivePath(current)) {
        throw new CoreError("Diese Funktion wird nicht unterstützt: Verknüpfungen folgen", "unsupported");
      }
      const target = node.entry.linkTarget ?? "";
      current = target.startsWith("/") ? normalize(target) : join(parentOf(current), target);
    }
    throw new CoreError(`Nicht gefunden: ${path}`, "notFound");
  };
  let activeSearch: { cancelled: boolean } | null = null;

  /** Dateizugriff für Plugin-Spalten und -Vorschau über dieselben Pfadregeln wie alle Befehle. */
  const pluginHost: PluginHost = {
    stat(path) {
      const node = find(resolvePath(path));
      return node ? { name: node.entry.name, dir: node.children !== undefined, size: node.entry.size } : null;
    },
    read(path, len) {
      const { node, name } = regularFile(path);
      return mockFileBytes(name, node.entry.size, 0, len);
    },
  };

  // ---------------------------------------------------------------- Suche über mehrere Dateien und Archive

  interface MockSearch {
    id: number;
    root: string;
    hits: SearchHit[];
    counters: Omit<
      SearchSummary,
      "root" | "total" | "incomplete" | "cancelled" | "stopped" | "issues"
    >;
    incomplete: boolean;
    cancelled: boolean;
    stopped: SearchStopReason | null;
    issues: string[];
  }
  const searches = new Map<number, MockSearch>();
  let runningSearches = 0;
  const searchGone = (): CoreError => new CoreError("Das Suchergebnis ist nicht (mehr) vorhanden", "notFound");
  const STOP_TEXT: Record<SearchStopReason, string> = {
    cancelled: "Die Suche wurde abgebrochen",
    hits: "Die Obergrenze für Treffer ist erreicht",
    files: "Die Obergrenze für Dateien ist erreicht",
    bytes: "Die Obergrenze für gelesene Bytes ist erreicht",
    time: "Die Zeitgrenze ist erreicht",
  };
  const archiveDepthOf = (path: string): number => (isArchivePath(path) ? 1 + archiveDepthOf(splitArchive(path).outer) : 0);
  const searchSummary = (search: MockSearch): SearchSummary => ({
    root: search.root,
    total: search.hits.length,
    ...search.counters,
    incomplete: search.incomplete,
    cancelled: search.cancelled,
    stopped: search.stopped,
    issues: [...search.issues],
  });

  const checkSearchOptions = (options: SearchOptions): void => {
    const number = (n: number | null | undefined): boolean => n == null || (Number.isSafeInteger(n) && n >= 0);
    if (!number(options.minSize) || !number(options.maxSize)) {
      throw new CoreError("Ungültige Eingabe: Größe", "invalidInput");
    }
    if (options.minSize != null && options.maxSize != null && options.minSize > options.maxSize) {
      throw new CoreError("Die Größenangaben widersprechen sich", "invalidInput");
    }
    for (const time of [options.modifiedAfter, options.modifiedBefore]) {
      if (time != null && !Number.isSafeInteger(time)) throw new CoreError("Ungültige Eingabe: Datum", "invalidInput");
    }
    if (options.modifiedAfter != null && options.modifiedBefore != null && options.modifiedAfter > options.modifiedBefore) {
      throw new CoreError("Die Datumsangaben widersprechen sich", "invalidInput");
    }
    const depth = options.archiveDepth ?? 1;
    if (!Number.isInteger(depth) || depth < 1 || depth > MAX_ARCHIVE_DEPTH) {
      throw new CoreError("Die Archivtiefe muss zwischen 1 und 3 liegen", "invalidInput");
    }
  };

  /** Die Suche selbst: ein Schritt (Zeitgeber, Abbruchpunkt) je Ordner. */
  const runSearch = async (job: Job, search: MockSearch, start: string, options: SearchOptions): Promise<void> => {
    const { name: nameRe, text: textRe } = compilePatterns(options);
    const remote = isRemotePath(start);
    const maxFiles = remote ? MOCK_SEARCH_REMOTE_FILES : Number.MAX_SAFE_INTEGER;
    const maxDepth = remote ? 16 : 128;
    const archiveLevels = remote || !options.inArchives ? 0 : Math.min(options.archiveDepth ?? 1, MAX_ARCHIVE_DEPTH - archiveDepthOf(start));
    const hasFilter = [options.minSize, options.maxSize, options.modifiedAfter, options.modifiedBefore].some((v) => v != null);
    const issue = (text: string): void => {
      search.incomplete = true;
      if (search.issues.length < 100) search.issues.push(text);
    };
    const stop = (reason: SearchStopReason): void => {
      if (search.stopped) return;
      search.stopped = reason;
      if (reason === "cancelled") search.cancelled = true;
      issue(STOP_TEXT[reason]);
    };
    const push = (hit: SearchHit): boolean => {
      if (search.hits.length >= MAX_SEARCH_HITS) {
        stop("hits");
        return false;
      }
      search.hits.push(hit);
      return true;
    };
    const passes = (entry: Entry): boolean => {
      if (options.minSize != null && entry.size < options.minSize) return false;
      if (options.maxSize != null && entry.size > options.maxSize) return false;
      if (options.modifiedAfter != null || options.modifiedBefore != null) {
        if (entry.modified == null) return false;
        if (options.modifiedAfter != null && entry.modified < options.modifiedAfter) return false;
        if (options.modifiedBefore != null && entry.modified > options.modifiedBefore) return false;
      }
      return true;
    };
    const nameOk = (name: string): boolean => (nameRe ? nameRe.test(name) : true);
    const nameHit = (path: string, entry: Entry): void => {
      push({ path, kind: entry.kind, size: entry.size, line: null, excerpt: null, matchStart: 0, matchLen: 0 });
    };
    const total = MOCK_SEARCH_TOTAL_BYTES;

    const readText = (path: string, entry: Entry): void => {
      if (entry.size === 0) return;
      const left = total - search.counters.bytesRead;
      if (left <= 0) {
        stop("bytes");
        return;
      }
      const want = Math.min(entry.size, MOCK_SEARCH_FILE_BYTES, left);
      const bytes = mockFileBytes(entry.name, entry.size, 0, want);
      search.counters.bytesRead += bytes.length;
      const scan = scanText(bytes, textRe!, MAX_SEARCH_HITS);
      if (scan.binary) {
        search.counters.skippedBinary += 1;
        return;
      }
      search.counters.filesSearched += 1;
      for (const hit of scan.hits) {
        if (!push({ path, kind: "file", size: entry.size, line: hit.line, excerpt: hit.excerpt, matchStart: hit.matchStart, matchLen: hit.matchLen })) return;
      }
      if (scan.moreHits) stop("hits");
      if (scan.cutLine) issue(`${path}: Zeilen über 65536 Bytes wurden nur zum Teil durchsucht`);
      if (entry.size > want) {
        if (want === left && left < MOCK_SEARCH_FILE_BYTES) stop("bytes");
        else issue(`${path}: Nur die ersten ${MOCK_SEARCH_FILE_BYTES} Bytes wurden durchsucht`);
      }
    };

    const walk = async (node: Node, dirPath: string, level: number): Promise<void> => {
      const stack: { node: Node; path: string; depth: number }[] = [{ node, path: dirPath, depth: 0 }];
      while (stack.length > 0) {
        const dir = stack.pop()!;
        if (search.stopped) return;
        await pause(job);
        search.counters.dirsVisited += 1;
        const subdirs: { node: Node; path: string; depth: number }[] = [];
        for (const name of [...(dir.node.children?.keys() ?? [])].sort()) {
          if (search.stopped) return;
          const child = dir.node.children!.get(name)!;
          const path = join(dir.path, name);
          const entry = child.entry;
          if (entry.error !== null) {
            issue(`${path}: ${entry.error}`);
            continue;
          }
          if (entry.kind === "dir") {
            if (!textRe && !hasFilter && nameRe && nameOk(name)) nameHit(path, entry);
            if (options.recursive ?? true) {
              if (dir.depth + 1 > maxDepth) issue(`${path}: Tiefengrenze erreicht`);
              else subdirs.push({ node: child, path, depth: dir.depth + 1 });
            }
          } else if (entry.kind === "symlink") {
            // Nie verfolgt oder gelesen; der Name kann trotzdem passen.
            if (!textRe && !hasFilter && nameRe && nameOk(name)) nameHit(path, entry);
          } else {
            if (search.counters.filesVisited + 1 > maxFiles) {
              stop("files");
              return;
            }
            search.counters.filesVisited += 1;
            let opened = false;
            if (child.archive && level < archiveLevels) {
              opened = true;
              search.counters.archivesSearched += 1;
              await walk(child.archive, formatArchive(path, "/"), level + 1);
              if (search.stopped) return;
            }
            if (nameOk(name) && passes(entry)) {
              if (!textRe) nameHit(path, entry);
              else if (!opened) readText(path, entry);
            }
          }
        }
        stack.push(...subdirs.reverse());
      }
    };
    await walk(needDir(start), start, 0);
  };

  // ---------------------------------------------------------------- Vergleichen und Abgleichen

  const TIME_TOLERANCE_MS = 2_000;
  const compares = new Map<number, MockCompare>();
  let runningCompares = 0;
  const syncPlans = new Map<string, MockSyncPlan>();
  let nextSyncPlan = 1;

  const compareGone = (): CoreError => new CoreError("Das Vergleichsergebnis ist nicht (mehr) vorhanden", "notFound");
  const planGone = (): CoreError =>
    new CoreError("Die Bestätigung gehört nicht zu diesem Synchronisierungsplan", "invalidToken");
  const checkRange = (offset: number, limit: number): void => {
    const ok = (n: number): boolean => Number.isSafeInteger(n) && n >= 0;
    if (!ok(offset) || !ok(limit)) throw new CoreError("Ungültige Eingabe: Bereich", "invalidInput");
  };
  const sideOf = (node: Node | undefined): CompareSide | null =>
    node
      ? { kind: node.entry.kind, size: node.entry.size, modified: node.entry.modified, linkTarget: node.entry.linkTarget }
      : null;
  const joinRel = (root: string, rel: string): string => rel.split("/").reduce((path, part) => join(path, part), root);

  /** `true`: links neuer, `false`: rechts neuer, `null`: innerhalb der Toleranz oder Zeit fehlt. */
  const newerSide = (left: number | null, right: number | null): boolean | null => {
    if (left === null || right === null) return null;
    const diff = left - right;
    return diff > TIME_TOLERANCE_MS ? true : diff < -TIME_TOLERANCE_MS ? false : null;
  };
  const stateByTime = (l: Node, r: Node): CompareState => {
    const newer = newerSide(l.entry.modified, r.entry.modified);
    return newer === null ? "different" : newer ? "leftNewer" : "rightNewer";
  };

  const comparePair = (l: Node, r: Node, mode: CompareMode): { state: CompareState; note: string | null } => {
    const [le, re] = [l.entry, r.entry];
    if (le.kind !== re.kind) return { state: "typeMismatch", note: null };
    if (le.kind === "dir") return { state: "same", note: null };
    if (le.kind === "symlink") return { state: le.linkTarget === re.linkTarget ? "same" : "different", note: null };
    if (mode === "sizeTime") {
      const newer = newerSide(le.modified, re.modified);
      if (newer !== null) return { state: newer ? "leftNewer" : "rightNewer", note: null };
      const unsure = le.size !== re.size || le.modified === null || re.modified === null;
      return { state: unsure ? "different" : "same", note: null };
    }
    if (le.size !== re.size) return { state: stateByTime(l, r), note: null };
    return { state: (l.version ?? 0) === (r.version ?? 0) ? "same" : stateByTime(l, r), note: null };
  };

  /** Wie `compare_dirs` im Kern: Paarung nach Namen, Fehler = unbekannt, nie „leer“. Ein Schritt je Ordner. */
  const walkCompare = async (job: Job, id: number, left: string, right: string, options: CompareOptions): Promise<MockCompare> => {
    const entries: CompareEntry[] = [];
    const issues: string[] = [];
    let incomplete = false;
    const issue = (text: string): void => {
      incomplete = true;
      if (issues.length < 100) issues.push(text);
    };
    interface Pending {
      rel: string;
      left: Node | null;
      right: Node | null;
      entry: CompareEntry | null;
    }
    const stack: Pending[] = [{ rel: "", left: needDir(left), right: needDir(right), entry: null }];
    while (stack.length > 0) {
      const dir = stack.pop()!;
      await pause(job);
      if (dir.entry) dir.entry.listed = true;
      const names = [...new Set([...(dir.left?.children?.keys() ?? []), ...(dir.right?.children?.keys() ?? [])])].sort();
      const children: Pending[] = [];
      for (const name of names) {
        const l = dir.left?.children?.get(name);
        const r = dir.right?.children?.get(name);
        const rel = dir.rel === "" ? name : `${dir.rel}/${name}`;
        const broken = l?.entry.error ?? r?.entry.error ?? null;
        let state: CompareState;
        let note: string | null = null;
        if (broken !== null) {
          state = "unknown";
          note = broken;
          issue(`${rel}: ${broken}`);
        } else if (l && !r) state = "onlyLeft";
        else if (!l && r) state = "onlyRight";
        else ({ state, note } = comparePair(l!, r!, options.mode));
        const entry: CompareEntry = { rel, state, left: sideOf(l), right: sideOf(r), note, listed: false };
        entries.push(entry);
        const lDir = l?.entry.kind === "dir";
        const rDir = r?.entry.kind === "dir";
        const descend = (lDir && rDir) || (lDir && !r) || (rDir && !l);
        if (descend && state !== "unknown" && options.recursive) {
          children.push({ rel, left: l ?? null, right: r ?? null, entry });
        }
      }
      stack.push(...children.reverse());
    }
    const byParts = (a: CompareEntry, b: CompareEntry): number => {
      const [pa, pb] = [a.rel.split("/"), b.rel.split("/")];
      for (let i = 0; i < Math.min(pa.length, pb.length); i += 1) {
        if (pa[i]! !== pb[i]!) return pa[i]! < pb[i]! ? -1 : 1;
      }
      return pa.length - pb.length;
    };
    entries.sort(byParts);
    return { id, left, right, mode: options.mode, recursive: options.recursive, entries, incomplete, cancelled: false, issues };
  };

  const countsOf = (entries: CompareEntry[]): CompareCounts => {
    const counts = Object.fromEntries(COMPARE_STATES.map((state) => [state, 0])) as CompareCounts;
    for (const entry of entries) counts[entry.state] += 1;
    return counts;
  };

  /** Zustand des Ziels beim Planen; vor jeder Aktion wird er mit dem aktuellen verglichen. */
  const versionOf = (node: Node): TargetVersion => ({
    node,
    kind: node.entry.kind,
    size: node.entry.size,
    modified: node.entry.modified,
    version: node.version ?? 0,
    linkTarget: node.entry.linkTarget,
    children: node.children ? [...node.children.keys()].sort() : [],
  });

  /** `null`: unverändert, sonst der Grund (wie `verify` im Kern). */
  const changedSince = (path: string, expected: TargetVersion): string | null => {
    const node = find(path);
    if (!node || node.entry.kind !== expected.kind || node !== expected.node) {
      return "Der Eintrag wurde seit der Planung ausgetauscht";
    }
    if (expected.kind === "dir") {
      const [first] = node.children ? [...node.children.keys()] : [];
      if (first === undefined) return null;
      return expected.children.includes(first)
        ? `Der Ordner enthält noch geplante Einträge (${first})`
        : `Im Ordner liegt ein neuer Eintrag (${first})`;
    }
    const same =
      node.entry.size === expected.size &&
      node.entry.modified === expected.modified &&
      (node.version ?? 0) === expected.version &&
      node.entry.linkTarget === expected.linkTarget;
    return same ? null : "Der Eintrag hat sich seit der Planung geändert";
  };

  const basePath = (path: string): string => (isArchivePath(path) ? basePath(splitArchive(path).outer) : path);

  /** Wie `plan_sync` im Kern: nur sicher Bekanntes, Löschen nur beim Spiegeln und nie bei unvollständigem Ergebnis. */
  const buildPlan = (result: MockCompare, mode: SyncMode, options: SyncOptions): MockSyncPlan => {
    const srcIsLeft = mode !== "rightToLeft";
    const source = srcIsLeft ? result.left : result.right;
    const target = srcIsLeft ? result.right : result.left;
    refuseArchiveWrite(target);
    if (isRemotePath(target)) {
      throw new CoreError("Abgleichen auf einen Server wird nicht unterstützt; der Abgleich bleibt lokal", "unsupported");
    }
    const [sb, tb] = [basePath(source), basePath(target)];
    if (sb === tb) throw new CoreError("Quelle und Ziel sind dieselbe Datei", "invalidTarget");
    if (sb.startsWith(`${tb}/`) || tb.startsWith(`${sb}/`) || tb === "/" || sb === "/") {
      throw new CoreError("Der Zielordner liegt innerhalb der Quelle", "invalidTarget");
    }
    const incomplete = result.incomplete || result.cancelled;
    const unknown = result.entries.filter((entry) => entry.state === "unknown").map((entry) => entry.rel);
    const blocked = (rel: string): boolean =>
      unknown.some((u) => u === "" || rel === u || rel.startsWith(`${u}/`));
    const childNames = new Map<string, Set<string>>();
    for (const entry of result.entries) {
      if (entry.rel === "") continue;
      const cut = entry.rel.lastIndexOf("/");
      const parent = cut < 0 ? "" : entry.rel.slice(0, cut);
      childNames.set(parent, (childNames.get(parent) ?? new Set()).add(entry.rel.slice(cut + 1)));
    }
    const actions: PlannedAction[] = [];
    const notes: SyncPlanNote[] = [];
    const note = (rel: string, reason: string): void => void notes.push({ rel, reason });
    const removals: { rel: string; info: CompareSide; listed: boolean }[] = [];
    for (const entry of result.entries) {
      const rel = entry.rel;
      if (rel === "" || blocked(rel)) continue;
      const [srcSide, dstSide] = srcIsLeft ? [entry.left, entry.right] : [entry.right, entry.left];
      const to = joinRel(target, rel);
      switch (entry.state) {
        case "unknown":
        case "same":
          break;
        case "typeMismatch":
          note(rel, "Datei, Ordner und Verknüpfung passen nicht zusammen; nicht automatisch angeglichen");
          break;
        case "onlyLeft":
        case "onlyRight":
          if (srcSide && !dstSide) {
            actions.push(
              srcSide.kind === "dir"
                ? { kind: "makeDir", rel, entryKind: "dir", size: 0, from: null, to, expected: null }
                : { kind: "copy", rel, entryKind: srcSide.kind, size: srcSide.size, from: joinRel(source, rel), to, expected: null },
            );
          } else if (!srcSide && dstSide && mode === "mirror") removals.push({ rel, info: dstSide, listed: entry.listed });
          break;
        default: {
          if (!srcSide || !dstSide) break;
          if (srcSide.kind !== "file" || dstSide.kind !== "file") {
            note(rel, "Verknüpfungen werden nicht ersetzt");
            break;
          }
          const sourceNewer = (entry.state === "leftNewer") === srcIsLeft && entry.state !== "different";
          if (!(options.replaceNewer && (sourceNewer || mode === "mirror"))) {
            note(
              rel,
              sourceNewer
                ? "Die Quelle ist neuer; nicht ersetzt (nur Fehlendes ergänzen)"
                : entry.state === "different"
                  ? "Gleiche Zeit, anderer Inhalt; nicht ersetzt"
                  : "Das Ziel ist neuer; nicht ersetzt",
            );
            break;
          }
          const node = find(to);
          if (node && node.entry.kind === "file") {
            actions.push({ kind: "replace", rel, entryKind: "file", size: srcSide.size, from: joinRel(source, rel), to, expected: versionOf(node) });
          } else note(rel, "Das Ziel ist keine Datei mehr");
        }
      }
    }
    if (incomplete) {
      if (mode === "mirror") note("", "Der Vergleich ist unvollständig; es wird nichts gelöscht");
    } else {
      // Eintrag für Eintrag, von unten nach oben (im sortierten Ergebnis folgen Kinder ihrem Ordner).
      for (const { rel, info, listed } of removals.reverse()) {
        if (info.kind === "dir" && !listed) {
          note(rel, "Der Ordnerinhalt wurde nicht erfasst; nicht gelöscht");
          continue;
        }
        const to = joinRel(target, rel);
        const node = find(to);
        if (!node) note(rel, "Eintrag nicht prüfbar, nicht gelöscht: nicht gefunden");
        else if (node.entry.kind !== info.kind) note(rel, "Der Eintrag hat seinen Typ geändert; nicht gelöscht");
        else if (
          node.children &&
          [...node.children.keys()].sort().join("\0") !== [...(childNames.get(rel) ?? [])].sort().join("\0")
        ) {
          note(rel, "Der Ordnerinhalt hat sich seit dem Vergleich geändert; nicht gelöscht");
        } else {
          actions.push({ kind: "trash", rel, entryKind: info.kind, size: info.kind === "file" ? info.size : 0, from: null, to, expected: versionOf(node) });
        }
      }
    }
    return {
      token: "",
      compareId: result.id,
      mode,
      source,
      target,
      actions,
      notes,
      incomplete,
    };
  };

  /** Behält die gewählten Aktionen samt den Ordnern, die sie brauchen (wie `select_actions`). */
  const selectActions = (plan: MockSyncPlan, selection: string[]): MockSyncPlan => {
    const rels = new Set(plan.actions.map((action) => action.rel));
    const unknown = selection.find((rel) => !rels.has(rel));
    if (unknown !== undefined) {
      throw new CoreError(`Die Auswahl enthält einen Eintrag, der nicht im Plan steht: ${unknown}`, "invalidInput");
    }
    const wanted = new Set(selection);
    const dirs = new Map(plan.actions.filter((a) => a.kind === "makeDir").map((a) => [a.rel, a]));
    const keep = new Set<PlannedAction>();
    for (const action of plan.actions) {
      if (!wanted.has(action.rel)) continue;
      keep.add(action);
      if (action.kind === "trash") continue;
      for (let up = action.rel; up.includes("/"); ) {
        up = up.slice(0, up.lastIndexOf("/"));
        const dir = dirs.get(up);
        if (dir) keep.add(dir);
      }
    }
    return { ...plan, actions: plan.actions.filter((action) => keep.has(action)) };
  };

  const toDtoAction = (action: PlannedAction, index: number): SyncAction => ({
    index,
    kind: action.kind,
    rel: action.rel,
    entryKind: action.entryKind,
    size: action.size,
  });

  const planDto = (plan: MockSyncPlan): SyncPlan => {
    const count = (kind: SyncActionKind): number => plan.actions.filter((action) => action.kind === kind).length;
    const actions = plan.actions.slice(0, SYNC_PLAN_PREVIEW).map(toDtoAction);
    return {
      token: plan.token,
      mode: plan.mode,
      source: plan.source,
      target: plan.target,
      makeDirs: count("makeDir"),
      copies: count("copy"),
      replaces: count("replace"),
      trashes: count("trash"),
      bytes: plan.actions.reduce((sum, a) => sum + (a.kind === "copy" || a.kind === "replace" ? a.size : 0), 0),
      totalActions: plan.actions.length,
      actions,
      actionsTruncated: plan.actions.length > actions.length,
      notes: plan.notes.slice(0, SYNC_PLAN_PREVIEW),
      notesTotal: plan.notes.length,
      incomplete: plan.incomplete,
    };
  };

  /** Führt die Aktionen eines bestätigten Plans aus; jede wird vorher gegen den Ist-Stand geprüft. */
  const runPlan = async (job: Job, plan: MockSyncPlan): Promise<void> => {
    const touched = new Set<string>([plan.target]);
    const skip = (path: string, reason: string): void =>
      publish(job.id, {
        skipped: [...statusOf(job.id).skipped, path],
        warnings: [...statusOf(job.id).warnings, { path, message: `Übersprungen: ${reason}` }],
      });
    const done = (bytes = 0): void =>
      publish(job.id, {
        state: "running",
        filesDone: statusOf(job.id).filesDone + 1,
        bytesDone: statusOf(job.id).bytesDone + bytes,
        bytesPerSecond: BYTES_PER_SECOND,
      });
    try {
      for (const action of plan.actions) {
        await pause(job);
        const parentPath = parentOf(action.to);
        const parent = find(parentPath);
        const name = basename(action.to);
        if (!parent?.children) {
          skip(action.to, "Der Zielordner ist nicht mehr vorhanden");
          continue;
        }
        touched.add(parentPath);
        const existing = parent.children.get(name);
        if (action.kind === "makeDir") {
          if (existing?.children) done();
          else if (existing) skip(action.to, "Am Ziel liegt inzwischen etwas anderes");
          else {
            parent.children.set(name, { entry: makeEntry(name, "dir", 0, clock()), children: new Map() });
            done();
          }
          continue;
        }
        if (action.kind === "trash") {
          const reason = changedSince(action.to, action.expected!);
          if (reason !== null) {
            skip(action.to, reason);
            continue;
          }
          parent.children.delete(name);
          let key = `trash://${job.id}/${name}`;
          for (let n = 2; trashed.has(key); n += 1) key = `trash://${job.id}/${name} (${n})`;
          trashed.set(key, { originalPath: action.to, node: existing! });
          done();
          continue;
        }
        const source = find(action.from!);
        if (!source || source.entry.kind !== action.entryKind || source.children) {
          skip(action.from!, source ? "Die Quelle hat ihren Typ geändert" : "Quelle nicht lesbar: nicht gefunden");
          continue;
        }
        if (action.kind === "copy") {
          if (existing) skip(action.to, "Am Ziel liegt inzwischen etwas");
          else {
            parent.children.set(name, cloneNode(source, name));
            done(source.entry.size);
          }
          continue;
        }
        const reason = changedSince(action.to, action.expected!);
        if (reason !== null) {
          skip(action.to, reason);
          continue;
        }
        // Geplantes Überschreiben: die Frage geht über die Konfliktbrücke, wie beim Kopieren.
        const answer = await askConflict(job, action.from!, action.to, source, existing!);
        if (answer.resolution === "cancel") throw new CancelledSignal();
        if (answer.resolution === "skip") {
          publish(job.id, { skipped: [...statusOf(job.id).skipped, action.from!] });
        } else {
          const finalName =
            answer.resolution === "keepBoth" ? keepBothName(name, (c) => parent.children?.has(c) ?? false) : name;
          parent.children.set(finalName, cloneNode(source, finalName));
          done(source.entry.size);
        }
      }
      publish(job.id, { state: statusOf(job.id).failed.length > 0 ? "failed" : "done", bytesPerSecond: 0 });
    } catch (error) {
      if (error instanceof CancelledSignal) publish(job.id, { state: "cancelled", bytesPerSecond: 0 });
      else {
        publish(job.id, {
          failed: [...statusOf(job.id).failed, { path: plan.target, message: error instanceof Error ? error.message : "Unbekannter Fehler" }],
        });
        publish(job.id, { state: "failed", bytesPerSecond: 0 });
      }
    } finally {
      touched.forEach(changed);
    }
  };

  /** Ein Server hat keinen Papierkorb; stattdessen wird nie still endgültig gelöscht. */
  const refuseServerTrash = (items: string[]): void => {
    if (items.some(isRemotePath)) {
      throw new CoreError("Auf einem Server gibt es keinen Papierkorb", "unsupported");
    }
  };

  // ---------------------------------------------------------------- API

  let startMenuExists = true;
  const api: MockApi = {
    ...createTerminalMock(),
    systemStatus: async () => ({ appImage: false, startMenuExists }),
    startMenuSet: async enabled => { startMenuExists = enabled; },
    traySet: async () => false,
    updateCheck: async () => null,
    updateCancel: async () => false,
    updateInstall: async () => { throw new CoreError("Self-update requires AppImage", "unsupported"); },
    restart: async () => {},
    releaseOpen: async () => {},
    ...createSpaceMock({
      resolve: resolvePath, entry: (path) => need(path).entry,
      list: (path) => [...(needDir(path).children?.values() ?? [])].map(n => n.entry),
      version: (path) => need(path).version ?? 0,
      start: (name, kind, run) => {
        const job = newJob(name, 0, null, { kind, first: "", more: 0 });
        void Promise.resolve().then(() => run(job.id)).then(() => publish(job.id, {state: "done"})).catch(error => {
          if(error instanceof CancelledSignal) publish(job.id, {state: "cancelled"});
          else {addFailure(job.id, "", String(error));publish(job.id, {state:"failed"});}
        });
        return job.id;
      },
      checkpoint: async id => { const job=jobs.get(id)!;await checkpoint(job);await pause(job); },
      report: (id, filesDone, bytesDone) => publish(id, {filesDone, bytesDone}),
      cancel: id => {void api.cancelJob(id);},
      trash: paths => api.trash(paths),
    }),
    ...createSimilarMock({
      cancelled: id => jobs.get(id)?.cancelled ?? true,
      resolve: resolvePath, entry: (path) => need(path).entry,
      list: (path) => [...(needDir(path).children?.values() ?? [])].map(n => n.entry),
      version: (path) => need(path).version ?? 0,
      start: (name, kind, run) => {
        const job = newJob(name, 0, null, { kind, first: "", more: 0 });
        void Promise.resolve().then(() => run(job.id)).then(() => publish(job.id, {state: "done"})).catch(error => {
          if(error instanceof CancelledSignal) publish(job.id, {state: "cancelled"});
          else {addFailure(job.id, "", String(error));publish(job.id, {state:"failed"});}
        });
        return job.id;
      },
      checkpoint: async id => { const job=jobs.get(id)!;await checkpoint(job);await pause(job); },
      report: (id, filesDone, bytesDone) => publish(id, {filesDone, bytesDone}),
      cancel: id => {void api.cancelJob(id);},
      trash: paths => api.trash(paths),
    }),

    requestClose() { const count = [...statuses.values()].filter(s => !isTerminal(s.state)).length; if (count) emit({ type: "closeRequested", count }); },
    async appCloseNow() {},
    async appConfirmClose() { await Promise.all([...statuses.values()].filter(s => !isTerminal(s.state)).map(s => api.cancelJob(s.id))); },
    async answerItemError(jobId, decision) {
      const answer = pendingErrors.get(jobId);
      if (!answer) throw new CoreError("No pending question", "notFound");
      pendingErrors.delete(jobId); answer(decision);
    },
    ...createMockSshPrompts(scheduler),
    async localDrives() { return [{ name: "USB-Stick", path: "/run/media/demo/USB-Stick" }, { name: "Sicherung", path: "/run/media/demo/Sicherung" }]; },
    async systemPlacesList() { return structuredClone(mockSystemPlaces).map(p => unmountedSystemPlaces.has(p.url) ? { ...p, mountedAt: null, sources: p.sources.filter(s => s !== "mounted") } : p); },
    async systemPlacesImport(urls) {
      if (urls.length > 200 || urls.some(url => !mockSystemPlaces.some(p => p.url === url))) throw new CoreError("Unknown place", "invalidInput");
      const newTargets = [...new Set(urls)].filter(url => !systemPlaceExists(mockSystemPlaces.find(p => p.url === url)!, [...profiles.values()].map(r => r.profile)));
      if (profiles.size + newTargets.length > MAX_PROFILES) throw new CoreError("Too many profiles", "invalidInput");
      for (const url of newTargets) {
        const place = mockSystemPlaces.find(p => p.url === url)!;
        if (systemPlaceExists(place, [...profiles.values()].map(r => r.profile))) continue;
        const profile = systemPlaceProfile(place);
        const base = profile.name; let suffix = 2;
        while ([...profiles.values()].some(r => r.profile.name.toLowerCase() === profile.name.toLowerCase())) profile.name = `${base} (${suffix++})`;
        await this.connectionSave(profile);
      }
      return this.connectionsList();
    },
    async systemMountConnect(profileId) {
      const profile = record(profileId).profile;
      if (profile.protocol !== "system") throw new CoreError("Invalid profile", "invalidInput");
      const place = mockSystemPlaces.find(p => p.url === profile.systemUrl);
      if (!place?.mountedAt) throw new CoreError("System mount unavailable", "systemMount");
      unmountedSystemPlaces.delete(place.url);
      return place.mountedAt;
    },
    async systemMountUnmount(profileId) {
      const profile = record(profileId).profile;
      if (profile.protocol !== "system" || !profile.systemUrl) throw new CoreError("Invalid profile", "invalidInput");
      unmountedSystemPlaces.add(profile.systemUrl);
    },
    async sshHostsList() { return ["backup-box", "fileserver", "nas", "web-vps"]; },
    async sshHostResolve(alias) {
      if (!/^[A-Za-z0-9._@:%[\]-]{1,255}$/.test(alias) || alias.startsWith("-")) throw new CoreError("Ungültiger Alias", "invalidInput");
      return { hostname: `${alias}.example`, user: "demo", port: 22, identityfile: ["~/.ssh/id_ed25519"] };
    },
    spaceFree: async () => null,
    async list(path): Promise<Listing> {
      const resolved = resolvePath(path);
      const node = needDir(resolved);
      if (node.entry.error) throw new CoreError(`Zugriff verweigert: ${resolved}`, "permission");
      return { path: resolved, entries: [...node.children!.values()].map((child) => snapshot(child, resolved)) };
    },

    async homeDir(): Promise<string> {
      return home;
    },

    async undoList() { return history.map((r) => ({ ...r.entry })); },
    undoRun: runUndo,
    onUndoChanged(handler) { undoHandlers.add(handler); return () => { undoHandlers.delete(handler); }; },
    async copy(sources, destination): Promise<number> {
      return startTransfer(sources, destination, false);
    },

    async move(sources, destination): Promise<number> {
      return startTransfer(sources, destination, true);
    },

    async packArchive(sources, destination, name, format): Promise<number> {
      if (!PACK_FORMATS.includes(format)) throw new CoreError("Ungültige Eingabe: Format", "invalidInput");
      if (sources.length === 0) throw new CoreError("Keine Einträge angegeben", "invalidPath");
      const paths = sources.map(resolvePath);
      const destinationPath = resolvePath(destination);
      refuseArchiveWrite(destinationPath);
      if (isRemotePath(destinationPath)) {
        throw new CoreError("Packen auf einen Server wird nicht unterstützt; bitte in einen lokalen Ordner packen", "unsupported");
      }
      validateName(name);
      const target = needDir(destinationPath);
      for (const path of paths) {
        if (isRootPath(path)) throw new CoreError("Die Wurzel kann nicht gepackt werden", "invalidPath");
      }
      if (paths.some((path) => placeOf(path) !== placeOf(paths[0]!))) {
        throw new CoreError("Die Einträge liegen nicht im selben Ordner oder Archiv", "invalidInput");
      }
      const nodes = paths.map(need);
      if (target.children?.has(name)) throw new CoreError(`Der Name ist bereits vorhanden: ${name}`, "exists");
      const job = newJob(describe("Packen", paths), countByKind(nodes).bytes, destinationLane(destinationPath), titleOf("pack", paths));

      const run = async (): Promise<void> => {
        const packed: Node[] = [];
        const names = new Set<string>();
        try {
          for (const path of paths) {
            await pause(job);
            const node = find(path);
            if (!node) {
              addFailure(job.id, path, "Nicht mehr vorhanden");
              continue;
            }
            if (node.children && (destinationPath === path || destinationPath.startsWith(`${path}/`))) {
              addFailure(job.id, path, "Der Zielordner liegt innerhalb der Quelle");
              continue;
            }
            if (names.has(node.entry.name)) {
              addFailure(job.id, path, `Der Name ist bereits vorhanden: ${node.entry.name}`);
              continue;
            }
            names.add(node.entry.name);
            const totals = countByKind([node]);
            packed.push(node);
            publish(job.id, {
              state: "running",
              bytesDone: statusOf(job.id).bytesDone + totals.bytes,
              filesDone: statusOf(job.id).filesDone + totals.files,
              bytesPerSecond: BYTES_PER_SECOND,
            });
          }
          if (packed.length > 0) {
            // Nie ein bestehendes Archiv ersetzen, auch nicht bei einem Wettlauf während des Auftrags.
            if (target.children?.has(name)) addFailure(job.id, join(destinationPath, name), `Der Name ist bereits vorhanden: ${name}`);
            else {
              const bytes = packed.reduce((sum, node) => sum + countByKind([node]).bytes, 0);
              const ratio = format === "zip" ? 0.6 : format === "tarGz" ? 0.55 : format === "tarBz2" ? 0.5 : 0.4;
              const archive: Node = {
                entry: makeEntry(name, "file", Math.max(100, Math.round(bytes * ratio)), clock()),
                archive: {
                  entry: makeEntry("", "dir", 0, clock()),
                  children: new Map(packed.map((node) => [node.entry.name, cloneNode(node, node.entry.name)])),
                },
              };
              target.children?.set(name, archive);
              changed(destinationPath);
            }
          }
          publish(job.id, { state: statusOf(job.id).failed.length > 0 ? "failed" : "done", bytesPerSecond: 0 });
        } catch (error) {
          if (error instanceof CancelledSignal) publish(job.id, { state: "cancelled", bytesPerSecond: 0 });
          else {
            addFailure(job.id, paths[0] ?? "", error instanceof Error ? error.message : "Unbekannter Fehler");
            publish(job.id, { state: "failed", bytesPerSecond: 0 });
          }
        }
      };
      void run();
      return job.id;
    },

    async trash(items): Promise<number> {
      refuseServerTrash(items);
      const paths = items.map(resolvePath);
      refuseArchiveWrite(...paths);
      for (const path of paths) {
        if (isRootPath(path)) throw new CoreError("Die Wurzel kann nicht entfernt werden", "invalidPath");
        need(path);
      }
      const job = newJob(describe("Papierkorb", paths), 0, null, titleOf("trash", paths));
      const pairs: [string, string][] = [];
      for (const path of paths) {
        const node = need(path);
        need(parentOf(path)).children?.delete(basename(path));
        let key = `trash://${job.id}/${basename(path)}`;
        for (let n = 2; trashed.has(key); n += 1) key = `trash://${job.id}/${basename(path)} (${n})`;
        trashed.set(key, { originalPath: path, node });
        pairs.push([path, key]);
        changed(parentOf(path));
      }
      recordUndo("trash", pairs);
      publish(job.id, { state: "done", filesDone: paths.length });
      return job.id;
    },

    // Wiederherstellen per Papierkorb-Schlüssel (`trash://<Auftrag>/<Name>`) oder Originalpfad.
    async restoreFromTrash(items): Promise<void> {
      refuseServerTrash(items);
      refuseArchiveWrite(...items);
      for (const item of items) {
        const key = trashed.has(item) ? item : [...trashed].find(([, value]) => value.originalPath === item)?.[0];
        const found = key === undefined ? undefined : trashed.get(key);
        if (key === undefined || !found) throw new CoreError(`Nicht im Papierkorb: ${item}`, "notFound");
        const parent = find(parentOf(found.originalPath));
        if (!parent?.children) {
          throw new CoreError(`Ursprünglicher Ordner fehlt: ${parentOf(found.originalPath)}`, "notFound");
        }
        const name = basename(found.originalPath);
        if (parent.children.has(name)) throw new CoreError(`Der Name ist bereits vorhanden: ${name}`, "exists");
        parent.children.set(name, found.node);
        trashed.delete(key);
        changed(parentOf(found.originalPath));
      }
    },

    async planDelete(items): Promise<DeletePlan> {
      const paths = items.map(resolvePath);
      refuseArchiveWrite(...paths);
      refusePluginWrite(...paths);
      const nodes = paths.map((path) => {
        if (isRootPath(path)) throw new CoreError("Die Wurzel kann nicht gelöscht werden", "invalidPath");
        return need(path);
      });
      const totals = countByKind(nodes);
      const token = `del-${nextPlan++}-${Math.random().toString(36).slice(2, 10)}`;
      const places = new Set(paths.map((path) => placeOf(path)));
      if (places.size > 1 && paths.some(isRemotePath)) {
        throw new CoreError("Die Einträge liegen nicht im selben Ordner oder Archiv", "invalidInput");
      }
      // Der Plan gehört zu genau dieser Verbindung (Token einer anderen oder neu aufgebauten ist ungültig).
      const connectionId =
        paths[0] !== undefined && isRemotePath(paths[0]) && !paths[0].startsWith("plugin:") ? splitRemote(paths[0]).id : null;
      const profileId = connectionId === null ? null : (live.get(connectionId)?.profileId ?? null);
      deletePlans.set(token, { paths, connectionId, profileId });
      return {
        token,
        files: totals.files,
        dirs: totals.dirs,
        links: totals.links,
        bytes: totals.bytes,
        firstNames: nodes.slice(0, 5).map((node) => node.entry.name),
        incomplete: false,
      };
    },

    async deletePermanent(plan, confirmedToken): Promise<number> {
      const planned = deletePlans.get(plan.token);
      if (confirmedToken !== plan.token || !planned) {
        throw new CoreError("Die Bestätigung gehört nicht zu diesem Löschplan", "invalidToken");
      }
      deletePlans.delete(plan.token); // jeder Plan gilt nur einmal
      const { paths } = planned;
      if (planned.connectionId !== null && !live.has(planned.connectionId)) {
        // Neu verbunden: die Bestätigung gehört zur alten Verbindung. Sonst einfach getrennt.
        if (planned.profileId !== null && byProfile.has(planned.profileId)) {
          throw new CoreError("Die Bestätigung gehört nicht zu diesem Löschplan", "invalidToken");
        }
        throw disconnected();
      }
      // Die Bestätigung gilt für genau diesen Umfang, nicht für später Hinzugekommenes.
      const nodes = paths.map(find);
      const present = nodes.filter((node): node is Node => node !== null);
      const totals = countByKind(present);
      if (
        present.length !== nodes.length ||
        totals.files !== plan.files ||
        totals.dirs !== plan.dirs ||
        totals.links !== plan.links
      ) {
        throw new CoreError("Der Umfang hat sich geändert, nichts wurde gelöscht", "planChanged");
      }
      const job = newJob(describe("Löschen", paths), totals.bytes, null, titleOf("delete", paths));
      for (const path of paths) {
        need(parentOf(path)).children?.delete(basename(path));
        changed(parentOf(path));
      }
      publish(job.id, { state: "done", filesDone: totals.files + totals.links, bytesDone: totals.bytes });
      return job.id;
    },

    async rename(path, newName): Promise<{ oldPath: string; newPath: string }> {
      const oldPath = resolvePath(path);
      refuseArchiveWrite(oldPath);
      refusePluginWrite(oldPath);
      validateName(newName);
      if (isRootPath(oldPath)) throw new CoreError("Die Wurzel kann nicht umbenannt werden", "invalidPath");
      const node = need(oldPath);
      const parent = need(parentOf(oldPath));
      const newPath = join(parentOf(oldPath), newName);
      if (newPath === oldPath) return { oldPath, newPath };
      if (parent.children?.has(newName)) throw new CoreError(`Der Name ist bereits vorhanden: ${newName}`, "exists");
      parent.children?.delete(basename(oldPath));
      node.entry = { ...node.entry, name: newName, hidden: newName.startsWith(".") };
      parent.children?.set(newName, node);
      recordUndo("rename", [[oldPath, newPath]]);
      changed(parentOf(oldPath));
      return { oldPath, newPath };
    },

    async renameMany(folder, items): Promise<number> {
      const folderPath = resolvePath(folder);
      refuseArchiveWrite(folderPath);
      refusePluginWrite(folderPath);
      const dir = needDir(folderPath);
      if (items.length === 0) throw new CoreError("Keine Einträge zum Umbenennen", "invalidInput");
      for (const item of items) {
        validateName(item.oldName);
        validateName(item.newName);
      }
      const olds = items.map((item) => join(folderPath, item.oldName));
      const job = newJob(describe("Umbenennen", olds), 0, null, titleOf("rename", olds));
      // Wie der Kern: Planfehler lehnen den ganzen Lauf ab, eine fehlende Quelle scheitert allein.
      const planFailures = new Map<string, string>();
      const seenOld = new Set<string>();
      const targetCount = new Map<string, number>();
      for (const item of items) {
        if (item.oldName !== item.newName) targetCount.set(item.newName, (targetCount.get(item.newName) ?? 0) + 1);
      }
      const movers: RenameItem[] = [];
      const missing: string[] = [];
      for (const item of items) {
        const oldPath = join(folderPath, item.oldName);
        if (seenOld.has(item.oldName)) planFailures.set(oldPath, "Doppelter Quellname in der Auswahl");
        seenOld.add(item.oldName);
        if (item.oldName === item.newName) continue;
        if ((targetCount.get(item.newName) ?? 0) > 1) {
          planFailures.set(oldPath, "Doppelter Zielname in der Auswahl");
        } else if (!dir.children?.has(item.oldName)) missing.push(oldPath);
        else movers.push(item);
      }
      const renamedNames = new Set(movers.map((item) => item.oldName));
      for (const item of movers) {
        if (!renamedNames.has(item.newName) && dir.children?.has(item.newName)) {
          planFailures.set(join(folderPath, item.oldName), `Der Name ist bereits vorhanden: ${item.newName}`);
        }
      }
      const failed = [
        ...missing.map((path) => ({ path, message: `Nicht gefunden: ${path}` })),
        ...[...planFailures].map(([path, message]) => ({ path, message })),
      ];
      if (planFailures.size > 0) {
        publish(job.id, { state: "failed", failed });
        return job.id;
      }
      // Zwei Phasen: erst alle aus dem Ordner nehmen, dann unter den neuen Namen einsetzen.
      const lifted = movers.map((item) => ({ item, node: dir.children!.get(item.oldName)! }));
      for (const { item } of lifted) dir.children!.delete(item.oldName);
      for (const { item, node } of lifted) {
        node.entry = { ...node.entry, name: item.newName, hidden: item.newName.startsWith(".") };
        dir.children!.set(item.newName, node);
      }
      if (!lifted.some(({ item }) => lifted.some(({ item: other }) => item.oldName === other.newName))) {
        recordUndo("rename", lifted.map(({ item }) => [join(folderPath, item.oldName), join(folderPath, item.newName)]));
      }
      if (lifted.length > 0) changed(folderPath);
      publish(job.id, {
        state: failed.length > 0 && lifted.length === 0 ? "failed" : "done",
        filesDone: lifted.length,
        failed,
      });
      return job.id;
    },

    async regexReplace(names, pattern, replacement, ignoreCase, all): Promise<RegexResult[]> {
      return mockRegexReplace(names, pattern, replacement, ignoreCase, all);
    },

    async makeDir(parent, name): Promise<string> {
      const parentPath = resolvePath(parent);
      refuseArchiveWrite(parentPath);
      refusePluginWrite(parentPath);
      validateName(name);
      const node = needDir(parentPath);
      if (node.children!.has(name)) throw new CoreError(`Der Name ist bereits vorhanden: ${name}`, "exists");
      node.children!.set(name, { entry: makeEntry(name, "dir", 0, clock()), children: new Map() });
      recordUndo("mkdir", [[join(parentPath, name), join(parentPath, name)]]);
      changed(parentPath);
      return join(parentPath, name);
    },

    async openUrl(): Promise<void> {},

    async logInfo() {
      return { path: "~/.local/state/dualfile/dualfile.log", bytes: 38_912 };
    },
    async logRead(anonymize) {
      const text = MOCK_LOG.replaceAll("{HOME}", "/home/demo").replaceAll("{SRV}", "192.168.2.40").replaceAll("{USR}", "demo");
      return anonymize ? text.replaceAll("/home/demo", "~").replaceAll("192.168.2.40", "<server-1>").replaceAll("demo", "<benutzer>") : text;
    },
    async logSave(dir) {
      return `${dir.replace(/\/$/, "")}/dualfile-log-20261116-1502.txt`;
    },
    async logOpenFolder(): Promise<void> {},
    async reportIssueUrl() {
      return "https://github.com/Bexxs75/dualfile/issues/new?title=&body=Dualfile";
    },
    async logFrontend(): Promise<void> {},

    async openExternal(path): Promise<void> {
      if (isRemotePath(path)) {
        throw new CoreError("Dateien auf einem Server können nicht direkt geöffnet werden; erst kopieren", "unsupported");
      }
      const resolved = resolvePath(path);
      if (isArchivePath(resolved)) {
        throw new CoreError("Inhalte eines Archivs können nicht direkt geöffnet werden; erst entpacken", "unsupported");
      }
      need(resolved);
    },

    async jobPause(id): Promise<void> {
      const job = jobs.get(id);
      if (!job || isTerminal(statusOf(id).state)) throw new CoreError("Der Auftrag kann nicht pausiert werden", "invalidInput");
      job.paused = true;
      publish(id, { state: "paused", bytesPerSecond: 0 });
      for (const pending of jobs.values()) pending.gate?.();
    },
    async jobResume(id): Promise<void> {
      const job = jobs.get(id);
      if (!job || isTerminal(statusOf(id).state)) throw new CoreError("Der Auftrag kann nicht fortgesetzt werden", "invalidInput");
      job.paused = false;
      publish(id, { state: "queued" });
      job.gate?.();
    },
    async cancelJob(id): Promise<void> {
      const job = jobs.get(id);
      if (!job) throw new CoreError(`Auftrag ${id} nicht gefunden`, "notFound");
      if (isTerminal(statusOf(id).state)) return;
      job.cancelled = true;
      pendingErrors.get(id)?.("abort"); pendingErrors.delete(id);
      job.gate?.();
      const warning = job.inFlight?.commit() ?? null; // fortsetzbare Teildatei samt Hinweis
      publish(id, {
        state: "cancelled",
        bytesPerSecond: 0,
        ...(warning ? { warnings: [...statusOf(id).warnings, warning] } : {}),
      });
      if (job.timer !== null) scheduler.clearTimeout(job.timer);
      job.timer = null;
      job.wake?.();
      job.conflict?.({ resolution: "cancel", applyToAll: false });
    },

    async answerConflict(jobId, answer): Promise<void> {
      const job = jobs.get(jobId);
      if (!job?.conflict) throw new CoreError(`Auftrag ${jobId} wartet auf keine Antwort`, "notFound");
      const resolve = job.conflict;
      job.conflict = null;
      resolve(answer);
    },

    async jobs(): Promise<JobStatus[]> {
      return [...statuses.values()].map((s) => structuredClone(s));
    },

    async loadSettings() {
      if (!settings) {
        const fresh: Settings = {
          version: 6,
          verifyPolicy: "auto",
          theme: "schiefer",
          accent: "#e0a45c",
          accentAutomatic: true,
          density: "normal",
          fontSize: 13,
          language: "auto",
          bookmarks: [],
          session: { windows: [], activeWindow: 0 },
        };
        return { settings: fresh, outcome: { kind: "fresh" as const } };
      }
      return { settings: structuredClone(settings), outcome: { kind: "loaded" as const } };
    },

    async saveSettings(next): Promise<void> {
      if (!["never", "auto", "always"].includes(next.verifyPolicy)) throw new CoreError("Ungültige Einstellungen: Prüfsumme", "invalidSettings");
      // Serverorte gehören nicht in die Einstellungen (dort steht `conn:<profil-id>!<pfad>`).
      const paths = [
        ...(next.bookmarks ?? []).map((bookmark) => bookmark.path),
        ...(next.session?.windows ?? []).flatMap((win) => win.tabs.map((tab) => tab.path)),
      ];
      if (paths.some((path) => /^s?ftp:/.test(path))) {
        throw new CoreError("Serverorte dürfen nicht in den Einstellungen stehen", "invalidSettings");
      }
      settings = structuredClone(next);
    },

    // Der Mock meldet Änderungen selbst über `dirChanged`; eine Ordnerbeobachtung gibt es nicht.
    async watch(): Promise<void> {},

    async fileInfo(path): Promise<FileInfo> {
      const { node, name } = regularFile(path);
      const { kind, mime } = mockKind(mockFileBytes(name, node.entry.size, 0, 4096), name);
      return { size: node.entry.size, kind, modified: node.entry.modified, mimeGuess: mime };
    },

    async mediaTags(paths): Promise<MediaTags[]> {
      // Sample tags by extension so the rename placeholders can be exercised without real files.
      const empty = (): MediaTags => ({ taken: "", camera: "", artist: "", title: "", album: "", track: 0 });
      return paths.map((p) => {
        const name = (p.split("/").pop() ?? "").toLowerCase();
        if (/\.(jpe?g|png|tiff?|heic|webp)$/.test(name)) {
          return { taken: "2021:07:15 14:32:08", camera: "Canon EOS R5", artist: "", title: "", album: "", track: 0 };
        }
        if (/\.(mp3|flac|m4a|ogg)$/.test(name)) {
          return { taken: "", camera: "", artist: "Beispielkünstler", title: "Beispieltitel", album: "Beispielalbum", track: 3 };
        }
        return empty();
      });
    },

    async readBlock(path, offset, len): Promise<Uint8Array> {
      const { node, name } = regularFile(path);
      if (!Number.isFinite(offset) || !Number.isFinite(len) || offset < 0 || len < 0) {
        throw new CoreError("Ungültige Eingabe: Bereich", "invalidInput");
      }
      return mockFileBytes(name, node.entry.size, offset, Math.min(len, MAX_BLOCK_BYTES));
    },

    // Gleiche Regeln wie `find_in_file` im Kern, nur mit kleinerem Streckenlimit.
    async findInFile(path, needle, start, direction, ignoreCase): Promise<SearchResult> {
      const { node, name } = regularFile(path);
      if (needle.length === 0 || needle.length > MAX_NEEDLE_BYTES) {
        throw new CoreError("Ungültige Eingabe: Der Suchtext ist leer oder zu lang", "invalidInput");
      }
      const size = node.entry.size;
      const mine = { cancelled: false };
      if (activeSearch) activeSearch.cancelled = true; // eine neuere Suche ersetzt die laufende
      activeSearch = mine;
      const fold = (bytes: Uint8Array): Uint8Array =>
        ignoreCase ? bytes.map((b) => (b >= 0x41 && b <= 0x5a ? b + 0x20 : b)) : bytes;
      const wanted = fold(needle);
      const overlap = needle.length - 1;
      const forward = direction === "forward";
      let edge = forward ? start : Math.min(start, size);
      let scanned = 0;
      try {
        for (;;) {
          await Promise.resolve(); // Abbrechen zwischen den Blöcken ermöglichen
          if (mine.cancelled) throw new CoreError("Der Vorgang wurde abgebrochen", "cancelled");
          const step = Math.min(MAX_BLOCK_BYTES, MOCK_SEARCH_LIMIT - scanned);
          const from = forward ? edge : Math.max(0, edge - step);
          const span = forward ? step : edge - from;
          const data = fold(mockFileBytes(name, size, from, span + overlap));
          const matches = (at: number): boolean => wanted.every((byte, i) => data[at + i] === byte);
          const last = Math.min(data.length - needle.length, span - 1);
          let hit = -1;
          if (forward) for (let at = 0; at <= last && hit < 0; at += 1) hit = matches(at) ? at : -1;
          else for (let at = last; at >= 0 && hit < 0; at -= 1) hit = matches(at) ? at : -1;
          if (hit >= 0) return { kind: "found", offset: from + hit };
          scanned += span;
          if (forward) {
            edge = from + span;
            if (edge >= size || data.length < span + overlap) return { kind: "notFound" };
          } else {
            edge = from;
            if (edge === 0) return { kind: "notFound" };
          }
          if (scanned >= MOCK_SEARCH_LIMIT) return { kind: "continue", next: edge };
        }
      } finally {
        if (activeSearch === mine) activeSearch = null;
      }
    },

    async cancelSearch(): Promise<void> {
      if (activeSearch) activeSearch.cancelled = true;
    },

    async compareDirs(left, right, options): Promise<number> {
      const leftPath = resolvePath(left);
      const rightPath = resolvePath(right);
      needDir(leftPath);
      needDir(rightPath);
      if (options.mode !== "sizeTime" && options.mode !== "content") {
        throw new CoreError("Ungültige Eingabe: Vergleichsart", "invalidInput");
      }
      if (compares.size + runningCompares >= MAX_COMPARES) {
        throw new CoreError(
          `Es sind schon ${MAX_COMPARES} Vergleiche offen; bitte erst einen schließen`,
          "invalidInput",
        );
      }
      runningCompares += 1;
      const job = newJob(`Vergleichen: ${basename(leftPath)} ↔ ${basename(rightPath)}`, 0);
      void (async (): Promise<void> => {
        try {
          const result = await walkCompare(job, job.id, leftPath, rightPath, options);
          compares.set(job.id, result);
          publish(job.id, { state: "done", filesDone: result.entries.length });
        } catch (error) {
          // Ein abgebrochener Vergleich hinterlässt kein Ergebnis.
          if (error instanceof CancelledSignal) publish(job.id, { state: "cancelled" });
          else {
            addFailure(job.id, leftPath, error instanceof Error ? error.message : "Unbekannter Fehler");
            publish(job.id, { state: "failed" });
          }
        } finally {
          runningCompares -= 1;
        }
      })();
      return job.id;
    },

    async comparePage(id, offset, limit, filter = {}): Promise<ComparePage> {
      checkRange(offset, limit);
      const text = filter.text ?? "";
      if (byteLength(text) > MAX_COMPARE_FILTER_BYTES || text.includes("\0")) {
        throw new CoreError("Der Filtertext ist ungültig", "invalidInput");
      }
      const states = filter.states?.length ? filter.states : null;
      if (states?.some((state) => !COMPARE_STATES.includes(state))) {
        throw new CoreError("Ungültige Eingabe: Zustand", "invalidInput");
      }
      const result = compares.get(id);
      if (!result) throw compareGone();
      const needle = text.toLowerCase();
      const matching = result.entries.filter(
        (entry) => (!states || states.includes(entry.state)) && (needle === "" || entry.rel.toLowerCase().includes(needle)),
      );
      return {
        id,
        offset,
        total: matching.length,
        entries: matching.slice(offset, offset + Math.min(Math.max(limit, 1), MAX_COMPARE_PAGE)).map((entry) => structuredClone(entry)),
        summary: {
          left: result.left,
          right: result.right,
          mode: result.mode,
          recursive: result.recursive,
          entries: result.entries.length,
          counts: countsOf(result.entries),
          incomplete: result.incomplete,
          cancelled: result.cancelled,
          issues: [...result.issues],
        },
      };
    },

    async compareClose(id): Promise<void> {
      if (!compares.delete(id)) throw compareGone();
      for (const [token, plan] of syncPlans) if (plan.compareId === id) syncPlans.delete(token);
    },

    async searchStart(root, options): Promise<number> {
      checkSearchOptions(options);
      compilePatterns(options);
      const start = resolvePath(root);
      if (isRemotePath(start) && !options.includeRemote) {
        throw new CoreError("Suche auf Servern und in Plugin-Dateisystemen (nicht freigegeben)", "unsupported");
      }
      needDir(start);
      if (searches.size + runningSearches >= MAX_SEARCHES) {
        throw new CoreError(
          `Es sind schon ${MAX_SEARCHES} Suchergebnisse offen; bitte erst eines schließen`,
          "invalidInput",
        );
      }
      runningSearches += 1;
      const job = newJob(`Suchen: ${basename(start)}`, 0, null, { kind: "search", first: basename(start), more: 0 });
      const search: MockSearch = {
        id: job.id,
        root: start,
        hits: [],
        counters: {
          dirsVisited: 0,
          filesVisited: 0,
          filesSearched: 0,
          archivesSearched: 0,
          skippedBinary: 0,
          skippedSpecial: 0,
          bytesRead: 0,
        },
        incomplete: false,
        cancelled: false,
        stopped: null,
        issues: [],
      };
      void (async (): Promise<void> => {
        try {
          await runSearch(job, search, start, options);
          searches.set(job.id, search);
          publish(job.id, { state: "done", filesDone: search.hits.length });
        } catch (error) {
          if (error instanceof CancelledSignal) {
            // Wie im Kern: ein abgebrochener Lauf behält, was er bis dahin gefunden hat.
            search.cancelled = true;
            search.incomplete = true;
            search.stopped = "cancelled";
            search.issues.push(STOP_TEXT.cancelled);
            searches.set(job.id, search);
            publish(job.id, { state: "cancelled" });
          } else {
            addFailure(job.id, start, error instanceof Error ? error.message : "Unbekannter Fehler");
            publish(job.id, { state: "failed" });
          }
        } finally {
          runningSearches -= 1;
        }
      })();
      return job.id;
    },

    async searchPage(id, offset, limit): Promise<SearchPage> {
      checkRange(offset, limit);
      const search = searches.get(id);
      if (!search) throw searchGone();
      const size = Math.min(Math.max(limit, 1), MAX_SEARCH_PAGE);
      return {
        id,
        offset,
        total: search.hits.length,
        hits: search.hits.slice(offset, offset + size).map((hit) => ({ ...hit })),
        summary: searchSummary(search),
      };
    },

    async searchClose(id): Promise<void> {
      if (!searches.delete(id)) throw searchGone();
    },

    async planSync(compareId, mode, options, selection): Promise<SyncPlan> {
      if (!["leftToRight", "rightToLeft", "mirror"].includes(mode)) {
        throw new CoreError("Ungültige Eingabe: Abgleichsart", "invalidInput");
      }
      const result = compares.get(compareId);
      if (!result) throw compareGone();
      let plan = buildPlan(result, mode, options);
      if (selection) plan = selectActions(plan, selection);
      plan.token = `sync-${nextSyncPlan++}-${Math.random().toString(36).slice(2, 10)}`;
      syncPlans.set(plan.token, plan);
      while (syncPlans.size > MAX_SYNC_PLANS) syncPlans.delete(syncPlans.keys().next().value!);
      return planDto(plan);
    },

    async syncPlanPage(token, offset, limit): Promise<SyncPlanPage> {
      checkRange(offset, limit);
      const plan = syncPlans.get(token);
      if (!plan) throw planGone();
      const size = Math.min(Math.max(limit, 1), MAX_COMPARE_PAGE);
      return {
        total: plan.actions.length,
        offset,
        actions: plan.actions.slice(offset, offset + size).map((action, i) => toDtoAction(action, offset + i)),
      };
    },

    async runSync(token): Promise<number> {
      const plan = syncPlans.get(token);
      if (!plan) throw planGone();
      syncPlans.delete(token); // jeder Plan gilt nur einmal
      refuseArchiveWrite(plan.target);
      const bytes = plan.actions.reduce((sum, a) => sum + (a.kind === "copy" || a.kind === "replace" ? a.size : 0), 0);
      const job = newJob(`Synchronisieren: ${basename(plan.source)}`, bytes, null, { kind: "sync", first: basename(plan.source), more: 0 });
      void runPlan(job, plan);
      return job.id;
    },

    async diffFiles(left, right, options): Promise<DiffResult> {
      const [l, r] = [regularFile(left), regularFile(right)];
      for (const { node } of [l, r]) {
        if (node.entry.size > DIFF_MAX_BYTES) throw new CoreError(MSG_DIFF_TOO_LARGE, "unsupported");
      }
      const bytes = ({ node, name }: { node: Node; name: string }): Uint8Array =>
        mockFileBytes(name, node.entry.size, 0, node.entry.size);
      return diffBytes(bytes(l), bytes(r), options);
    },

    // ------------------------------------------------------------ Server-Verbindungen

    async connectionsList(): Promise<ConnectionEntry[]> {
      return [...profiles.values()].map(entryOf);
    },

    async connectionSave(input): Promise<ConnectionEntry> {
      const clean = validateProfile(input);
      const existing = typeof input.id === "string" && input.id !== "" ? record(input.id) : null;
      const sameName = [...profiles.values()].some(
        (rec) => rec.profile.id !== existing?.profile.id && rec.profile.name.toLowerCase() === clean.name.toLowerCase(),
      );
      if (sameName) throw new CoreError("Ungültige Eingabe: Der Name ist schon vergeben", "invalidInput");
      if (!existing && profiles.size >= MAX_PROFILES) {
        throw new CoreError(`Es sind höchstens ${MAX_PROFILES} Verbindungsprofile möglich`, "invalidInput");
      }
      const id = existing ? existing.profile.id : `profile-${nextProfile++}`;
      const profile = { ...clean, id };
      // Pins gelten je (Protokoll, Host, Port): ein geänderter Server erbt kein Vertrauen.
      const pins = existing?.profile.protocol === "sftp" && profile.protocol === "sftp" && (existing.profile.transport === "openssh" || profile.transport === "openssh") ? existing.pins : (existing?.pins ?? []).filter(
        (pin) => pin.protocol === profile.protocol && pin.host.toLowerCase() === profile.host.toLowerCase() && pin.port === profile.port,
      );
      if (existing && (existing.profile.transport ?? "builtin") !== profile.transport) closeConnection(id);
      const rec: ProfileRecord = { profile, pins };
      profiles.set(id, rec);
      pending.delete(id);
      if (!profile.saveSecret) secrets.delete(id);
      return entryOf(rec);
    },

    async connectionDelete(profileId): Promise<void> {
      record(profileId);
      closeConnection(profileId);
      pending.delete(profileId);
      secrets.delete(profileId);
      profiles.delete(profileId);
    },

    // Unbekannte Hosts melden `remote` (der Server ist nicht erreichbar bzw. kennt das Konto nicht).
    async connectionConnect(profileId, secret): Promise<ConnectOutcome> {
      const rec = record(profileId);
      const { profile } = rec;
      if (profile.protocol === "system") return { state: "connected", connectionId: profileId, root: await this.systemMountConnect(profileId), secretStore: "none" };
      pending.delete(profileId); // eine neue Anfrage ersetzt eine offene Bestätigung
      const existingId = byProfile.get(profileId);
      const existing = existingId === undefined ? undefined : live.get(existingId);
      if (existingId !== undefined && existing) {
        return { state: "connected", connectionId: existingId, root: existing.root, secretStore: secrets.get(profileId)?.kind ?? "none" };
      }
      if (profile.protocol === "ftp" && !profile.allowPlaintext) {
        throw new CoreError("Unverschlüsseltes FTP ist für dieses Profil nicht erlaubt", "invalidInput");
      }
      if (live.size >= MAX_CONNECTIONS) {
        throw new CoreError(`Es sind schon ${MAX_CONNECTIONS} Verbindungen offen; bitte erst eine trennen`, "tooManyConnections");
      }
      if (profile.transport === "openssh") return finishConnect(rec, undefined);
      const host = profile.host.toLowerCase();
      if (host === "timeout-host") throw new CoreError("Der Server hat nicht rechtzeitig geantwortet", "timeout");
      if (host === "down-host") throw new CoreError("Der Server hat den Vorgang nicht ausgeführt", "remote");
      if (host === "pin-changed" && profile.protocol !== "ftp") {
        const old = pinFor(rec)?.sha256 ?? DEMO_FINGERPRINT_SFTP;
        return { state: "pinMismatch", old, new: DEMO_FINGERPRINT_CHANGED };
      }
      if (!DEMO_HOSTS[host]?.includes(profile.protocol)) {
        throw new CoreError("Der Server hat den Vorgang nicht ausgeführt", "remote");
      }
      if (profile.protocol === "ftp" || profile.protocol === "smb") return finishConnect(rec, secret); // ohne TLS kein Fingerabdruck
      const fingerprint = profile.protocol === "sftp" ? DEMO_FINGERPRINT_SFTP : DEMO_FINGERPRINT_FTPS;
      const pin = pinFor(rec);
      if (pin && pin.sha256 !== fingerprint) return { state: "pinMismatch", old: pin.sha256, new: fingerprint };
      if (pin) return finishConnect(rec, secret);
      // Erst die Vertrauensfrage; die Anmeldung wird erst nach der Bestätigung geprüft.
      pending.set(profileId, { fingerprint, secret });
      return profile.protocol === "sftp"
        ? { state: "needsTrust", fingerprint, keyType: "ssh-ed25519", kind: "newHostKey", changed: false }
        : { state: "needsTrust", fingerprint, keyType: null, kind: "unknownIssuer", changed: false };
    },

    async connectionTrust(profileId, decision: TrustDecision, fingerprint): Promise<ConnectOutcome> {
      const rec = record(profileId);
      const waiting = pending.get(profileId);
      pending.delete(profileId); // auch bei einem Fehler ist die Bestätigung verbraucht
      if (!waiting) throw new CoreError("Ungültige Eingabe: Es gibt nichts zu bestätigen", "invalidInput");
      if (decision !== "once" && decision !== "always") throw new CoreError("Ungültige Eingabe: Entscheidung", "invalidInput");
      if (fingerprint !== waiting.fingerprint) {
        throw new CoreError("Ungültige Eingabe: Der Fingerabdruck passt nicht zur Anfrage", "invalidInput");
      }
      if (decision === "always" && (rec.profile.protocol === "ftps" || rec.profile.protocol === "sftp")) {
        const pin: PinInfo = {
          protocol: rec.profile.protocol,
          host: rec.profile.host,
          port: rec.profile.port,
          sha256: waiting.fingerprint,
          keyType: rec.profile.protocol === "sftp" ? "ssh-ed25519" : null,
        };
        rec.pins = [...rec.pins.filter((old) => old !== pinFor(rec)), pin];
      }
      return finishConnect(rec, waiting.secret);
    },

    async connectionResetTrust(profileId): Promise<void> {
      const rec = record(profileId);
      if (rec.profile.protocol === "system" || rec.profile.transport === "openssh") throw new CoreError("OpenSSH verwaltet known_hosts selbst", "invalidInput");
      rec.pins = [];
      pending.delete(profileId);
    },

    async connectionDisconnect(profileId): Promise<void> {
      record(profileId);
      closeConnection(profileId);
      pending.delete(profileId);
    },

    async secretSet(profileId, secret): Promise<SecretStoreKind> {
      if (record(profileId).profile.protocol === "system" || record(profileId).profile.transport === "openssh") throw new CoreError("OpenSSH speichert keine Antworten", "invalidInput");
      if (typeof secret !== "string") throw new CoreError("Ungültige Eingabe: Geheimnis", "invalidInput");
      secrets.set(profileId, { value: secret, kind: secretKind });
      return secretKind;
    },

    async secretClear(profileId): Promise<void> {
      record(profileId);
      secrets.delete(profileId);
    },

    async pluginsList() {
      return plugins.list();
    },
    async pluginActivate(id, manifestHash, grantWrite, unsandboxed, confirmUnsandboxed) {
      return plugins.activate(id, manifestHash, grantWrite, unsandboxed, confirmUnsandboxed);
    },
    async pluginDeactivate(id) {
      return plugins.deactivate(id);
    },
    async pluginTrustReset(id) {
      plugins.trustReset(id);
    },
    async pluginDiagnostics(id) {
      return plugins.diagnostics(id);
    },
    async pluginColumns(paths, columnIds) {
      return plugins.columns(paths, columnIds, pluginHost);
    },
    async pluginPreview(path, pluginId) {
      return plugins.preview(path, pluginId, pluginHost);
    },

    dropConnection(connectionId): void {
      const connection = live.get(connectionId);
      if (!connection) return;
      live.delete(connectionId);
      if (byProfile.get(connection.profileId) === connectionId) byProfile.delete(connection.profileId);
    },

    onEvent(handler): () => void {
      handlers.add(handler);
      return () => {
        handlers.delete(handler);
      };
    },
  };
  return api;
}
