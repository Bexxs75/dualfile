import type { Entry } from "../ipc/types";

/**
 * Platzhalter-Vergleich über den ganzen Namen, `*` = beliebig viele, `?` = genau ein Zeichen,
 * ohne Groß-/Kleinschreibung. Bewusst ohne RegExp: kein Escaping nötig, keine Backtracking-Explosion
 * bei Mustern, die ein Nutzer ins Filterfeld tippt.
 */
function globMatch(name: string, pattern: string): boolean {
  const text = [...name.toLowerCase()];
  const pat = [...pattern.toLowerCase()];
  let t = 0;
  let p = 0;
  let star = -1;
  let resume = 0;
  while (t < text.length) {
    if (p < pat.length && (pat[p] === "?" || pat[p] === text[t])) {
      t++;
      p++;
    } else if (p < pat.length && pat[p] === "*") {
      star = p++;
      resume = t;
    } else if (star >= 0) {
      p = star + 1;
      t = ++resume;
    } else {
      return false;
    }
  }
  while (p < pat.length && pat[p] === "*") p++;
  return p === pat.length;
}

const hasWildcard = (s: string): boolean => s.includes("*") || s.includes("?");

function splitPatterns(pattern: string): string[] {
  return pattern
    .split(";")
    .map((part) => part.trim())
    .filter((part) => part !== "");
}

/**
 * Muster für `+`/`-`: mehrere durch `;` getrennt. Ohne Platzhalter muss der Name genau passen
 * (wie im Total Commander); ein leeres Muster trifft nichts.
 */
export function matchPattern(name: string, pattern: string): boolean {
  return splitPatterns(pattern).some((part) => globMatch(name, part));
}

/**
 * Filterfeld: Teilstring ohne Groß-/Kleinschreibung; enthält ein Teil `*` oder `?`, gilt er als Muster
 * für den ganzen Namen. ".." bleibt immer sichtbar (sonst käme man nicht mehr hoch), versteckte
 * Einträge nur bei `showHidden`.
 */
export function filterEntries<T extends Entry>(entries: readonly T[], text: string, showHidden: boolean): T[] {
  const parts = splitPatterns(text);
  return entries.filter((entry) => {
    if (entry.name === "..") return true;
    if (entry.hidden && !showHidden) return false;
    if (parts.length === 0) return true;
    const lower = entry.name.toLowerCase();
    return parts.some((part) => (hasWildcard(part) ? globMatch(entry.name, part) : lower.includes(part.toLowerCase())));
  });
}
