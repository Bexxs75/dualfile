// Mehrfach-Umbenennen: reine Berechnung der neuen Namen und der Vorschau. Reihenfolge fest:
// Maske (Name und Endung getrennt) -> Suchen/Ersetzen -> Groß/Klein -> Prüfung. Reguläre Ausdrücke
// werden hier nie ausgewertet, sondern vom Aufrufer über `api.regexReplace` (Kern, lineare Laufzeit)
// beschafft und als `RegexOutcome` übergeben.
import type { TranslationKey, TranslationParams } from "../i18n";
import type { EntryKind, MediaTags, RegexResult, RenameItem } from "../ipc/types";
import { nameError } from "./names";

export type CaseMode = "keep" | "upper" | "lower" | "words" | "first";
export type SearchMode = "text" | "regex";

export interface MultiRenameRules {
  /** Maske für den Namen ohne Endung; bei Ordnern für den ganzen Namen. */
  nameMask: string;
  /** Maske für die Endung (ohne Punkt); bei Ordnern ohne Wirkung. */
  extMask: string;
  counterStart: number;
  counterStep: number;
  /** Mindestzahl der Stellen des Zählers, links mit Nullen aufgefüllt. */
  counterDigits: number;
  search: string;
  replace: string;
  searchMode: SearchMode;
  ignoreCase: boolean;
  /** true = alle Treffer, false = nur der erste. */
  all: boolean;
  caseMode: CaseMode;
}

export const defaultRules: MultiRenameRules = {
  nameMask: "[N]",
  extMask: "[E]",
  counterStart: 1,
  counterStep: 1,
  counterDigits: 1,
  search: "",
  replace: "",
  searchMode: "text",
  ignoreCase: false,
  all: true,
  caseMode: "keep",
};

/** Grenzen für den Zähler (alles darüber ist eine Fehleingabe, kein Überlauf). */
export const COUNTER_MAX = 999_999_999;
export const COUNTER_MAX_DIGITS = 9;

export interface RenameSource {
  name: string;
  kind: EntryKind;
  /** Änderungszeit in Millisekunden seit 1970 oder null. */
  modified: number | null;
  /** EXIF-/ID3-Tags für `[X..]`/`[T..]`; leere Werte, solange nichts geladen ist. */
  tags: MediaTags;
}

export const emptyTags: MediaTags = { taken: "", camera: "", artist: "", title: "", album: "", track: 0 };

export interface MultiRenameInput {
  /** In der Reihenfolge der Dateiliste; danach richtet sich der Zähler. */
  sources: readonly RenameSource[];
  rules: MultiRenameRules;
  /** Name des übergeordneten Ordners für `[P]` (leer für die Wurzel). */
  parentName: string;
  /** Alle Namen des Ordners (auch die nicht ausgewählten, auch versteckte). */
  existingNames: readonly string[];
  /** true, solange die EXIF-/ID3-Tags noch geladen werden: Masken mit `[X..]`/`[T..]` sperren. */
  tagsPending?: boolean;
}

/** True, wenn eine Maske einen EXIF-/ID3-Platzhalter (`[X..]`/`[T..]`) nutzt; die braucht geladene Tags. */
export function maskUsesTags(mask: string): boolean {
  for (let i = 0; i < mask.length; i += 1) {
    if (mask.charAt(i) !== "[") continue;
    if (mask.charAt(i + 1) === "[") {
      i += 1;
      continue;
    }
    const close = mask.indexOf("]", i + 1);
    if (close < 0) break;
    const first = mask.charAt(i + 1);
    if ((first === "X" || first === "T") && mask.charAt(i + 1) !== "=") return true;
    i = close;
  }
  return false;
}

export type RowStatus = "unchanged" | "ok" | "error";

export interface PreviewRow {
  oldName: string;
  /** Der berechnete Name; bei einem Fehler vor der Namensbildung leer. */
  newName: string;
  status: RowStatus;
  errorKey?: TranslationKey;
  errorParams?: TranslationParams;
}

/** Antwort des Kerns auf die Regex-Ersetzung (oder ihr Scheitern im Ganzen, etwa ungültiges Muster). */
export type RegexOutcome = { kind: "results"; results: readonly RegexResult[] } | { kind: "failed"; message: string };

export interface RenamePlan {
  rows: PreviewRow[];
  /** Fehler der Einstellungen selbst (Zähler, Regex im Ganzen); gilt für alle Zeilen. */
  rulesError: { key: TranslationKey; params?: TranslationParams } | null;
  counts: { ok: number; unchanged: number; error: number };
  /** Nur die tatsächlich zu ändernden Einträge, bereit für `api.renameMany`; leer bei Fehlern. */
  items: RenameItem[];
  /** Alles fehlerfrei und mindestens ein Eintrag ändert sich. */
  canApply: boolean;
}

// ---------------------------------------------------------------- Namen zerlegen

/** Endung wie in der Dateiliste: ".bashrc", "Makefile" und "name." haben keine. */
export function splitName(name: string, kind: EntryKind): { base: string; ext: string } {
  if (kind === "dir") return { base: name, ext: "" };
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return { base: name, ext: "" };
  return { base: name.slice(0, dot), ext: name.slice(dot + 1) };
}

// ---------------------------------------------------------------- Maske

class MaskError extends Error {
  constructor(
    readonly key: TranslationKey,
    readonly params: TranslationParams = {},
  ) {
    super(key);
  }
}

interface MaskContext {
  base: string;
  ext: string;
  index: number;
  modified: number | null;
  tags: MediaTags;
  parentName: string;
  rules: MultiRenameRules;
}

const two = (value: number): string => String(value).padStart(2, "0");

/** `a-b`, `a-`, `-b`, `a` (1-basiert, einschließlich) auf Codepunkte; Rest ist der Bereichsteil hinter N/E. */
function sliceRange(text: string, range: string, token: string): string {
  const chars = Array.from(text);
  const match = /^(\d*)(-?)(\d*)$/.exec(range);
  if (!match || range === "" || range === "-") throw new MaskError("multirename.error.placeholder", { token });
  const [, from, dash, to] = match as unknown as [string, string, string, string];
  if (dash === "") {
    const at = Number(from);
    if (at < 1) throw new MaskError("multirename.error.range", { token });
    return chars.slice(at - 1, at).join("");
  }
  const start = from === "" ? 1 : Number(from);
  const end = to === "" ? Number.POSITIVE_INFINITY : Number(to);
  if (start < 1 || end < start) throw new MaskError("multirename.error.range", { token });
  return chars.slice(start - 1, end === Number.POSITIVE_INFINITY ? undefined : end).join("");
}

function counterText(context: MaskContext): string {
  const { counterStart, counterStep, counterDigits } = context.rules;
  const value = counterStart + context.index * counterStep;
  if (value < 0 || value > COUNTER_MAX) throw new MaskError("multirename.error.counterRange");
  return String(value).padStart(counterDigits, "0");
}

function dateText(letter: string, context: MaskContext, token: string): string {
  if (context.modified === null) throw new MaskError("multirename.error.noDate", { token });
  const date = new Date(context.modified);
  switch (letter) {
    case "Y":
      return String(date.getFullYear()).padStart(4, "0");
    case "M":
      return two(date.getMonth() + 1);
    case "D":
      return two(date.getDate());
    case "h":
      return two(date.getHours());
    case "m":
      return two(date.getMinutes());
    default:
      return two(date.getSeconds());
  }
}

/** Tag-Werte dürfen keine Pfadtrenner oder Zeilenumbrüche in den Dateinamen bringen. */
const safe = (text: string): string => text.replace(/[/\\\r\n]/g, "").trim();

function exifText(letter: string, context: MaskContext, token: string): string {
  const match = /^(\d{4}):(\d{2}):(\d{2})[ T](\d{2}):(\d{2}):(\d{2})/.exec(context.tags.taken);
  if (!match) throw new MaskError("multirename.error.noExifDate", { token });
  const index = "YMDhms".indexOf(letter);
  if (index < 0) throw new MaskError("multirename.error.placeholder", { token });
  return match[index + 1] as string;
}

function placeholder(token: string, context: MaskContext): string {
  if (token.startsWith("=")) return token.slice(1);
  const letter = token.charAt(0);
  const rest = token.slice(1);
  if (letter === "N" || letter === "E") {
    const text = letter === "N" ? context.base : context.ext;
    return rest === "" ? text : sliceRange(text, rest, token);
  }
  if (rest === "") {
    if (letter === "C") return counterText(context);
    if ("YMDhms".includes(letter) && letter !== "") return dateText(letter, context, token);
    if (letter === "P") return context.parentName;
  }
  if (letter === "X" && rest.length === 1) return rest === "c" ? safe(context.tags.camera) : exifText(rest, context, token);
  if (letter === "T" && rest.length === 1) {
    switch (rest) {
      case "a":
        return safe(context.tags.artist);
      case "t":
        return safe(context.tags.title);
      case "b":
        return safe(context.tags.album);
      case "n":
        return String(context.tags.track);
    }
  }
  throw new MaskError("multirename.error.placeholder", { token });
}

/** Wertet eine Maske aus; wirft `MaskError` bei unbekanntem oder kaputtem Platzhalter. */
function evaluateMask(mask: string, context: MaskContext): string {
  let out = "";
  for (let i = 0; i < mask.length; i += 1) {
    const char = mask.charAt(i);
    if (char === "[") {
      if (mask.charAt(i + 1) === "[") {
        out += "[";
        i += 1;
        continue;
      }
      const close = mask.indexOf("]", i + 1);
      if (close < 0) throw new MaskError("multirename.error.unclosed");
      out += placeholder(mask.slice(i + 1, close), context);
      i = close;
    } else if (char === "]") {
      if (mask.charAt(i + 1) !== "]") throw new MaskError("multirename.error.unopened");
      out += "]";
      i += 1;
    } else out += char;
  }
  return out;
}

// ---------------------------------------------------------------- Suchen/Ersetzen (Text), Groß/Klein

/** Textersetzung ohne reguläre Ausdrücke: Vergleich über Codepunkte, optional ohne Beachtung der Groß-/Kleinschreibung. */
export function replaceText(name: string, search: string, replacement: string, ignoreCase: boolean, all: boolean): string {
  if (search === "") return name;
  const fold = (char: string): string => (ignoreCase ? char.toLowerCase() : char);
  const hay = Array.from(name);
  const needle = Array.from(search).map(fold);
  let out = "";
  let i = 0;
  let replaced = false;
  while (i < hay.length) {
    const hit = !(replaced && !all) && i + needle.length <= hay.length && needle.every((c, k) => fold(hay[i + k] ?? "") === c);
    if (hit) {
      out += replacement;
      i += needle.length;
      replaced = true;
    } else {
      out += hay[i];
      i += 1;
    }
  }
  return out;
}

const isWordChar = (char: string): boolean => /[\p{L}\p{N}]/u.test(char);

export function applyCase(name: string, mode: CaseMode): string {
  switch (mode) {
    case "keep":
      return name;
    case "upper":
      return name.toUpperCase();
    case "lower":
      return name.toLowerCase();
    case "first": {
      const chars = Array.from(name.toLowerCase());
      const [head, ...tail] = chars;
      return head === undefined ? "" : head.toUpperCase() + tail.join("");
    }
    case "words": {
      let out = "";
      let previousWord = false;
      for (const char of Array.from(name)) {
        const word = isWordChar(char);
        out += word ? (previousWord ? char.toLowerCase() : char.toUpperCase()) : char;
        previousWord = word;
      }
      return out;
    }
  }
}

// ---------------------------------------------------------------- Regeln prüfen

/** Fehler der Einstellungen (Zähler), unabhängig von den Einträgen. */
export function rulesProblem(rules: MultiRenameRules): TranslationKey | null {
  const whole = (value: number): boolean => Number.isInteger(value);
  if (!whole(rules.counterStart) || !whole(rules.counterStep) || !whole(rules.counterDigits)) return "multirename.error.counterInvalid";
  if (rules.counterStart < 0 || rules.counterStart > COUNTER_MAX) return "multirename.error.counterInvalid";
  if (Math.abs(rules.counterStep) > 1_000_000) return "multirename.error.counterInvalid";
  if (rules.counterDigits < 1 || rules.counterDigits > COUNTER_MAX_DIGITS) return "multirename.error.counterInvalid";
  return null;
}

/** Braucht die Vorschau eine Antwort des Kerns (reguläre Ausdrücke)? */
export const needsRegex = (rules: MultiRenameRules): boolean => rules.searchMode === "regex" && rules.search !== "";

interface Stage {
  /** Name nach der jeweiligen Stufe, null bei Fehler. */
  name: string | null;
  error?: { key: TranslationKey; params?: TranslationParams };
}

function maskStage(input: MultiRenameInput): Stage[] {
  const { rules, parentName } = input;
  return input.sources.map((source, index): Stage => {
    const { base, ext } = splitName(source.name, source.kind);
    const context: MaskContext = { base, ext, index, modified: source.modified, tags: source.tags, parentName, rules };
    try {
      const name = evaluateMask(rules.nameMask, context);
      if (source.kind === "dir") return { name };
      const extension = evaluateMask(rules.extMask, context);
      return { name: extension === "" ? name : `${name}.${extension}` };
    } catch (error) {
      if (error instanceof MaskError) return { name: null, error: { key: error.key, params: error.params } };
      throw error;
    }
  });
}

/** Namen, die nach der Maske an den Kern zur Regex-Ersetzung gehen (nur Einträge ohne Fehler). */
export function regexTargets(input: MultiRenameInput): string[] {
  return maskStage(input).flatMap((stage) => (stage.name === null ? [] : [stage.name]));
}

/**
 * Berechnet die Vorschau. `regex` wird gebraucht, wenn `needsRegex(rules)`; fehlt es (noch keine
 * Antwort), bleibt die Ersetzung unberücksichtigt, und der Aufrufer darf nicht umbenennen lassen.
 */
export function planRename(input: MultiRenameInput, regex?: RegexOutcome): RenamePlan {
  const { rules, sources } = input;
  const problem = rulesProblem(rules);
  if (problem) {
    const rows = sources.map((s): PreviewRow => ({ oldName: s.name, newName: "", status: "error", errorKey: problem }));
    return { rows, rulesError: { key: problem }, counts: { ok: 0, unchanged: 0, error: rows.length }, items: [], canApply: false };
  }
  // Masken mit EXIF-/ID3-Platzhaltern dürfen nicht angewandt werden, solange die Tags noch laden
  // (sonst entstünden Namen aus leeren Werten); nach dem Laden greift die normale Zeilenprüfung.
  if (input.tagsPending && (maskUsesTags(rules.nameMask) || maskUsesTags(rules.extMask))) {
    const rows = sources.map((s): PreviewRow => ({ oldName: s.name, newName: "", status: "error", errorKey: "multirename.error.tagsLoading" }));
    return { rows, rulesError: { key: "multirename.error.tagsLoading" }, counts: { ok: 0, unchanged: 0, error: rows.length }, items: [], canApply: false };
  }

  const stages = maskStage(input);
  // Suchen/Ersetzen
  if (needsRegex(rules)) {
    if (regex?.kind === "failed") {
      const rows = sources.map(
        (s): PreviewRow => ({ oldName: s.name, newName: "", status: "error", errorKey: "multirename.error.regex", errorParams: { message: regex.message } }),
      );
      return {
        rows,
        rulesError: { key: "multirename.error.regex", params: { message: regex.message } },
        counts: { ok: 0, unchanged: 0, error: rows.length },
        items: [],
        canApply: false,
      };
    }
    if (regex?.kind === "results") {
      let at = 0;
      for (const stage of stages) {
        if (stage.name === null) continue;
        const result = regex.results[at];
        at += 1;
        if (!result) continue;
        if (result.kind === "ok") stage.name = result.name;
        else {
          stage.error = { key: "multirename.error.regex", params: { message: result.message } };
          stage.name = null;
        }
      }
    }
  } else if (rules.search !== "") {
    for (const stage of stages) {
      if (stage.name !== null) stage.name = replaceText(stage.name, rules.search, rules.replace, rules.ignoreCase, rules.all);
    }
  }
  // Groß/Klein
  for (const stage of stages) if (stage.name !== null) stage.name = applyCase(stage.name, rules.caseMode);

  // Einzelprüfung
  const rows: PreviewRow[] = sources.map((source, index): PreviewRow => {
    const stage = stages[index] as Stage;
    if (stage.name === null) {
      return { oldName: source.name, newName: "", status: "error", errorKey: stage.error?.key, errorParams: stage.error?.params };
    }
    const invalid = nameError(stage.name, []);
    if (invalid) return { oldName: source.name, newName: stage.name, status: "error", errorKey: invalid };
    return { oldName: source.name, newName: stage.name, status: stage.name === source.name ? "unchanged" : "ok" };
  });

  // Doppelte Ziele und Kollisionen mit Einträgen, die ihren Namen behalten. Wer selbst umbenannt wird,
  // gibt seinen alten Namen frei (Tausch, Ketten).
  const movers = rows.filter((row) => row.status === "ok");
  const freed = new Set(movers.map((row) => row.oldName));
  const staying = new Set(input.existingNames.filter((name) => !freed.has(name)));
  const targetCount = new Map<string, number>();
  for (const row of movers) targetCount.set(row.newName, (targetCount.get(row.newName) ?? 0) + 1);
  for (const row of movers) {
    if ((targetCount.get(row.newName) ?? 0) > 1) {
      row.status = "error";
      row.errorKey = "multirename.error.duplicate";
    } else if (staying.has(row.newName)) {
      row.status = "error";
      row.errorKey = "error.name.exists";
    }
  }

  const counts = {
    ok: rows.filter((row) => row.status === "ok").length,
    unchanged: rows.filter((row) => row.status === "unchanged").length,
    error: rows.filter((row) => row.status === "error").length,
  };
  const canApply = counts.error === 0 && counts.ok > 0;
  const items = canApply ? rows.filter((row) => row.status === "ok").map((row) => ({ oldName: row.oldName, newName: row.newName })) : [];
  return { rows, rulesError: null, counts, items, canApply };
}

// ---------------------------------------------------------------- Voreinstellungen

export interface RenamePreset {
  name: string;
  rules: MultiRenameRules;
}

export const MAX_PRESETS = 50;
export const MAX_PRESET_NAME = 80;
const MAX_FIELD = 1000;
const CASE_MODES: readonly CaseMode[] = ["keep", "upper", "lower", "words", "first"];

/**
 * Prüft ein Roh-Objekt (z. B. aus gespeicherten Daten) und liefert gültige Regeln oder null.
 * Typen, Längen und Zahlenbereiche werden einzeln geprüft; nichts wird blind übernommen.
 */
export function parseRules(raw: unknown): MultiRenameRules | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  const text = (key: string): string | null => (typeof r[key] === "string" && (r[key] as string).length <= MAX_FIELD ? (r[key] as string) : null);
  const num = (key: string): number | null => (typeof r[key] === "number" && Number.isFinite(r[key]) ? (r[key] as number) : null);
  const bool = (key: string): boolean | null => (typeof r[key] === "boolean" ? (r[key] as boolean) : null);
  const nameMask = text("nameMask");
  const extMask = text("extMask");
  const search = text("search");
  const replace = text("replace");
  const counterStart = num("counterStart");
  const counterStep = num("counterStep");
  const counterDigits = num("counterDigits");
  const ignoreCase = bool("ignoreCase");
  const all = bool("all");
  const searchMode = r.searchMode === "text" || r.searchMode === "regex" ? r.searchMode : null;
  const caseMode = CASE_MODES.find((mode) => mode === r.caseMode) ?? null;
  if (
    nameMask === null || extMask === null || search === null || replace === null || counterStart === null || counterStep === null ||
    counterDigits === null || ignoreCase === null || all === null || searchMode === null || caseMode === null
  ) {
    return null;
  }
  const rules: MultiRenameRules = { nameMask, extMask, counterStart, counterStep, counterDigits, search, replace, searchMode, ignoreCase, all, caseMode };
  return rulesProblem(rules) === null ? rules : null;
}

/** Bereinigt eine Liste von Voreinstellungen: ungültige fallen weg, Namen sind eindeutig, Anzahl begrenzt. */
export function sanitizePresets(raw: unknown): RenamePreset[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const out: RenamePreset[] = [];
  for (const item of raw as unknown[]) {
    if (typeof item !== "object" || item === null) continue;
    const entry = item as Record<string, unknown>;
    const name = typeof entry.name === "string" ? entry.name.trim() : "";
    const rules = parseRules(entry.rules);
    if (name === "" || name.length > MAX_PRESET_NAME || rules === null || seen.has(name)) continue;
    seen.add(name);
    out.push({ name, rules });
    if (out.length >= MAX_PRESETS) break;
  }
  return out;
}

/** Fügt eine Voreinstellung ein oder ersetzt die gleichnamige; ohne gültigen Namen unverändert. */
export function savePreset(presets: readonly RenamePreset[], name: string, rules: MultiRenameRules): RenamePreset[] {
  const trimmed = name.trim();
  if (trimmed === "" || trimmed.length > MAX_PRESET_NAME) return [...presets];
  const without = presets.filter((preset) => preset.name !== trimmed);
  if (without.length >= MAX_PRESETS) return [...presets];
  return [...without, { name: trimmed, rules: { ...rules } }];
}

export const deletePreset = (presets: readonly RenamePreset[], name: string): RenamePreset[] => presets.filter((preset) => preset.name !== name);

/** Platzhalter für das Einfügemenü: Text, der in die Maske kommt, und Schlüssel der Erklärung. */
export const PLACEHOLDERS: readonly { insert: string; label: TranslationKey }[] = [
  { insert: "[N]", label: "multirename.ph.name" },
  { insert: "[N1-3]", label: "multirename.ph.nameRange" },
  { insert: "[E]", label: "multirename.ph.ext" },
  { insert: "[C]", label: "multirename.ph.counter" },
  { insert: "[Y]", label: "multirename.ph.year" },
  { insert: "[M]", label: "multirename.ph.month" },
  { insert: "[D]", label: "multirename.ph.day" },
  { insert: "[h]", label: "multirename.ph.hour" },
  { insert: "[m]", label: "multirename.ph.minute" },
  { insert: "[s]", label: "multirename.ph.second" },
  { insert: "[XY]", label: "multirename.ph.exifYear" },
  { insert: "[XM]", label: "multirename.ph.exifMonth" },
  { insert: "[XD]", label: "multirename.ph.exifDay" },
  { insert: "[Xc]", label: "multirename.ph.exifCamera" },
  { insert: "[Ta]", label: "multirename.ph.id3Artist" },
  { insert: "[Tt]", label: "multirename.ph.id3Title" },
  { insert: "[Tb]", label: "multirename.ph.id3Album" },
  { insert: "[Tn]", label: "multirename.ph.id3Track" },
  { insert: "[P]", label: "multirename.ph.parent" },
  { insert: "[=Text]", label: "multirename.ph.literal" },
  { insert: "[[", label: "multirename.ph.openBracket" },
  { insert: "]]", label: "multirename.ph.closeBracket" },
];
