import de from "./de";
import en from "./en";

export type Language = "de" | "en";
export type LanguageSetting = Language | "auto";

type DictionaryKey = keyof typeof de;
type PluralSuffix = ".one" | ".other";
type PluralBase<K> = K extends `${infer Base}${PluralSuffix}` ? Base : never;
/** Alle Schlüssel für `t`; Mehrzahlformen werden über ihren Grundnamen und `count` abgerufen. */
export type TranslationKey = Exclude<DictionaryKey, `${string}${PluralSuffix}`> | PluralBase<DictionaryKey>;
export type TranslationParams = Record<string, string | number>;

const dictionaries: Record<Language, Record<string, string>> = { de, en };

/** Systemsprache auflösen: nur Deutsch wird erkannt, alles andere fällt auf Englisch. */
export function resolveLanguage(setting: LanguageSetting, navigatorLanguage: string | undefined): Language {
  if (setting !== "auto") return setting;
  return (navigatorLanguage ?? "").toLowerCase().startsWith("de") ? "de" : "en";
}

/**
 * Übersetzt einen Schlüssel. Platzhalter `{name}` kommen aus `params`; ist `params.count` eine
 * Zahl, wird die Mehrzahlform (`key.one` / `key.other`) gewählt. Ein fehlender Platzhalter
 * bleibt sichtbar stehen, damit der Fehler auffällt.
 */
export function t(key: TranslationKey, params: TranslationParams = {}, language: Language = "de"): string {
  const dictionary = dictionaries[language];
  let template: string | undefined;
  if (typeof params.count === "number") {
    const form = new Intl.PluralRules(language).select(params.count);
    template = dictionary[`${key}.${form === "one" ? "one" : "other"}`];
  }
  template ??= dictionary[key];
  if (template === undefined) return key;
  return template.replace(/\{(\w+)\}/g, (placeholder, name: string) => {
    const value = params[name];
    return value === undefined ? placeholder : String(value);
  });
}

/** Bindet die Sprache, damit Komponenten nur noch `tr("schlüssel", { … })` schreiben. */
export function createTranslator(language: Language): (key: TranslationKey, params?: TranslationParams) => string {
  return (key, params) => t(key, params, language);
}

/** Meldungstext zu einer Fehlerart des Kerns (`CoreError.kind`); unbekannte Arten → allgemeiner Text. */
export function errorKey(kind: string): TranslationKey {
  switch (kind) {
    case "smbSecurity": return "error.smbSecurity";
    case "smbUnreachable": return "error.smbUnreachable";
    case "smbShareMissing": return "error.smbShareMissing";
    case "smbAuth": return "error.smbAuth";
    case "smbFailure": return "error.smbFailure";

    case "systemMountTimeout": return "error.systemMountTimeout";
    case "systemMountFailed": return "error.systemMountFailed";
    case "systemMount": return "error.systemMount";
    case "notFound":
      return "error.notFound";
    case "invalidName":
      return "error.invalidName";
    case "exists":
      return "error.exists";
    case "permission":
      return "error.permission";
    case "notDirectory":
      return "error.notDirectory";
    case "invalidTarget":
      return "error.invalidTarget";
    case "invalidToken":
      return "error.invalidToken";
    case "planChanged":
      return "error.planChanged";
    case "cancelled":
      return "error.cancelled";
    case "readOnly":
      return "error.readOnly";
    case "archive":
      return "error.archive";
    case "io":
      return "error.io";
    case "disconnected":
    case "auth":
    case "invalidInput":
    case "opensshMissing":
    case "hostKeyRejected":
    case "unreachable":
    case "subsystemMissing":

    case "timeout":
    case "trust":
    case "algorithmNotAllowed":
    case "remote":
    case "sizeMismatch":
    case "pinMismatch":
    case "tooManyConnections":
    case "secretRemovalFailed":
    case "pluginLocked":
    case "pluginSandboxUnavailable":
    case "pluginTimeout":
    case "pluginProtocol":
    case "pluginDisabled":
    case "pluginCrashed":
    case "pluginTooLarge":
    case "pluginNotInstalled":
    case "pluginUnsafe":
    case "pluginBusy":
    case "pluginFailed":
      return `error.${kind}`;
    default:
      return "error.unknown";
  }
}

/** Menschenlesbare Größe (1024er-Schritte) mit Dezimalkomma im Deutschen. */
export function formatBytes(bytes: number, language: Language = "de"): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  const number = new Intl.NumberFormat(language, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  return `${number.format(value)} ${units[unit]}`;
}

/** Datum und Uhrzeit: `12.11.2026 08:15` (de) beziehungsweise `2026-11-12 08:15` (en). */
export function formatDate(milliseconds: number | null, language: Language = "de", timeZone?: string): string {
  if (milliseconds === null) return "";
  const parts = new Intl.DateTimeFormat("en-GB", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
    ...(timeZone ? { timeZone } : {}),
  }).formatToParts(new Date(milliseconds));
  const part = (type: string): string => parts.find((p) => p.type === type)?.value ?? "";
  const time = `${part("hour")}:${part("minute")}`;
  return language === "de"
    ? `${part("day")}.${part("month")}.${part("year")} ${time}`
    : `${part("year")}-${part("month")}-${part("day")} ${time}`;
}
