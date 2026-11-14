// Bausteine der Suche für den Mock (`mock.ts`): Muster, Zeilensuche und Auszug nach dem Vorbild
// von `crates/dualfile-core/src/search`. NUR ENTWURF: ausgewertet wird mit der JavaScript-Regex; das
// Produkt nutzt die lineare Rust-Engine (keine Rückverweise, keine Lookarounds), daher wird beides
// hier schon in der Syntax abgelehnt.
import { CoreError } from "./api";
import { MAX_SEARCH_EXCERPT_CHARS, MAX_SEARCH_PATTERN_BYTES } from "./types";
import type { SearchOptions } from "./types";

/** Binär-Erkennung: ein NUL in den ersten Bytes. */
export const BINARY_PROBE_BYTES = 8 * 1024;
/** Längste Zeile, die durchsucht wird (der Rest einer längeren Zeile nicht, mit Meldung). */
export const MAX_LINE_BYTES = 64 * 1024;
const EXCERPT_LEAD = 40;

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8");

const invalid = (message: string): CoreError => new CoreError(message, "invalidInput");

/** Übersetzt ein Glob (ohne `;`-Trennung) in Regex-Syntax für den `u`-Modus. */
export function globSource(glob: string): string {
  const chars = Array.from(glob);
  let out = "";
  const escape = (c: string): string => (/[\\^$.*+?()[\]{}|/]/.test(c) ? `\\${c}` : c);
  for (let i = 0; i < chars.length; i += 1) {
    const c = chars[i]!;
    if (c === "*") out += ".*";
    else if (c === "?") out += ".";
    else if (c === "\\" && i + 1 < chars.length) {
      i += 1;
      out += escape(chars[i]!);
    } else if (c === "[") {
      let j = i + 1;
      const negated = chars[j] === "!" || chars[j] === "^";
      if (negated) j += 1;
      const bodyStart = j;
      if (chars[j] === "]") j += 1;
      while (j < chars.length && chars[j] !== "]") j += 1;
      if (j >= chars.length) out += "\\[";
      else {
        out += negated ? "[^" : "[";
        for (const m of chars.slice(bodyStart, j)) out += /[\\[\]^]/.test(m) ? `\\${m}` : m;
        out += "]";
        i = j;
      }
    } else out += escape(c);
  }
  return out;
}

export interface Patterns {
  name: RegExp | null;
  text: RegExp | null;
}

const filled = (value: string | null | undefined): value is string => typeof value === "string" && value !== "";

function build(source: string, caseSensitive: boolean): RegExp {
  if (/\(\?<?[=!]|(?<!\\)(?:\\\\)*\\[1-9]|\\k</.test(source)) {
    throw invalid("Ungültiger regulärer Ausdruck (Rückverweise und Lookarounds gibt es nicht)");
  }
  try {
    return new RegExp(source.replaceAll("(?P<", "(?<"), `su${caseSensitive ? "" : "i"}`);
  } catch {
    throw invalid("Ungültiger regulärer Ausdruck");
  }
}

/** Prüft die Muster wie `check_options` im Kern und baut die Ausdrücke. */
export function compilePatterns(options: SearchOptions): Patterns {
  for (const text of [options.name, options.text]) {
    if (typeof text === "string" && (encoder.encode(text).length > MAX_SEARCH_PATTERN_BYTES || text.includes("\0"))) {
      throw invalid("Das Suchmuster ist ungültig oder zu lang");
    }
  }
  if (!filled(options.name) && !filled(options.text)) {
    throw invalid("Bitte ein Namensmuster oder einen Suchtext angeben");
  }
  let name: RegExp | null = null;
  if (filled(options.name)) {
    if ((options.nameMode ?? "glob") === "regex") name = build(options.name, options.nameCaseSensitive ?? false);
    else {
      const parts = options.name.split(";").filter((part) => part !== "");
      if (parts.length > 0) {
        name = build(`^(?:${parts.map(globSource).join("|")})$`, options.nameCaseSensitive ?? false);
      }
    }
  }
  let text: RegExp | null = null;
  if (filled(options.text)) {
    const source = options.textRegex ? options.text : options.text.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&");
    text = build(source, options.textCaseSensitive ?? false);
  }
  if (!name && !text) throw invalid("Bitte ein Namensmuster oder einen Suchtext angeben");
  return { name, text };
}

/** Der Auszug einer Zeile: Steuerzeichen ersetzt, auf `MAX_SEARCH_EXCERPT_CHARS` um den Treffer gekürzt. */
export function excerptOf(
  line: string,
  startUnit: number,
  endUnit: number,
): { excerpt: string; matchStart: number; matchLen: number } {
  const chars = Array.from(line);
  const startChar = Array.from(line.slice(0, startUnit)).length;
  const lenChars = Array.from(line.slice(startUnit, endUnit)).length;
  const from = chars.length <= MAX_SEARCH_EXCERPT_CHARS || startChar <= EXCERPT_LEAD ? 0 : startChar - EXCERPT_LEAD;
  const to = Math.min(from + MAX_SEARCH_EXCERPT_CHARS, chars.length);
  const body = chars
    .slice(from, to)
    // eslint-disable-next-line no-control-regex
    .map((c) => (/[\u0000-\u001f\u007f-\u009f]/.test(c) ? " " : c))
    .join("");
  const prefix = from > 0 ? 1 : 0;
  const rel = startChar - from;
  return {
    excerpt: `${from > 0 ? "…" : ""}${body}${to < chars.length ? "…" : ""}`,
    matchStart: prefix + rel,
    matchLen: Math.min(lenChars, Math.max(0, to - from - rel)),
  };
}

export interface TextHit {
  line: number;
  excerpt: string;
  matchStart: number;
  matchLen: number;
}

export interface TextScan {
  /** NUL in den ersten Bytes: nicht durchsucht. */
  binary: boolean;
  hits: TextHit[];
  /** Mindestens eine Zeile war länger als `MAX_LINE_BYTES` und wurde nur zum Teil durchsucht. */
  cutLine: boolean;
  /** Es gab mehr Treffer als `maxHits`. */
  moreHits: boolean;
}

/** Sucht Zeile für Zeile; höchstens `maxHits` Treffer (ein weiterer setzt `moreHits`). */
export function scanText(bytes: Uint8Array, re: RegExp, maxHits: number): TextScan {
  const result: TextScan = { binary: false, hits: [], cutLine: false, moreHits: false };
  if (bytes.subarray(0, BINARY_PROBE_BYTES).includes(0)) {
    result.binary = true;
    return result;
  }
  let lineNumber = 0;
  let start = 0;
  while (start <= bytes.length) {
    let end = bytes.indexOf(10, start);
    const last = end < 0;
    if (last) end = bytes.length;
    if (!(last && end === start)) {
      lineNumber += 1;
      let slice = bytes.subarray(start, end);
      if (slice.length > MAX_LINE_BYTES) {
        slice = slice.subarray(0, MAX_LINE_BYTES);
        result.cutLine = true;
      }
      let line = decoder.decode(slice);
      if (line.endsWith("\r")) line = line.slice(0, -1);
      re.lastIndex = 0;
      const found = re.exec(line);
      if (found) {
        if (result.hits.length >= maxHits) {
          result.moreHits = true;
          return result;
        }
        result.hits.push({ line: lineNumber, ...excerptOf(line, found.index, found.index + found[0].length) });
      }
    }
    if (last) break;
    start = end + 1;
  }
  return result;
}
