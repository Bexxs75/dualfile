// Zeilenvergleich für die Mock-API: gleiche Aufteilung, Schlüssel und Abschnittsform wie `diff_text`
// im Kern (`compare/diff.rs`). Die Suche nach dem kürzesten Unterschied ist hier einfacher (LCS-Tabelle
// mit Obergrenze); für Demo-Dateien reicht das, der Kern nutzt Myers.
import { CoreError } from "./api";
import { DIFF_MAX_BYTES, DIFF_MAX_LINES, MSG_DIFF_BINARY, MSG_DIFF_TOO_LARGE } from "./types";
import type { DiffHunk, DiffOptions, DiffResult } from "./types";

const BINARY_PROBE = 8 * 1024;
/** Größte LCS-Tabelle (Zellen); darüber wird der abweichende Mittelteil als ein Block gemeldet. */
const MAX_TABLE_CELLS = 4_000_000;

interface Lines {
  text: string[];
  keys: string[];
}

function splitLines(data: Uint8Array, options: DiffOptions): Lines {
  if (data.length > DIFF_MAX_BYTES) throw new CoreError(MSG_DIFF_TOO_LARGE, "unsupported");
  if (data.subarray(0, BINARY_PROBE).includes(0)) throw new CoreError(MSG_DIFF_BINARY, "unsupported");
  const content = new TextDecoder("utf-8").decode(data);
  const text: string[] = [];
  const keys: string[] = [];
  let rest = content;
  while (rest !== "") {
    const cut = rest.indexOf("\n");
    const line = cut < 0 ? rest : rest.slice(0, cut);
    const ended = cut >= 0;
    rest = cut < 0 ? "" : rest.slice(cut + 1);
    if (text.length >= DIFF_MAX_LINES) throw new CoreError(MSG_DIFF_TOO_LARGE, "unsupported");
    let key = options.ignoreWhitespace ? line.replace(/\s/gu, "") : line;
    if (options.ignoreCase) key = key.toLowerCase();
    // Eine letzte Zeile ohne Zeilenumbruch unterscheidet sich von derselben Zeile mit Umbruch.
    if (!ended && !options.ignoreWhitespace) key += "\n";
    text.push(line);
    keys.push(key);
  }
  return { text, keys };
}

type Op = "equal" | "delete" | "insert";

function editScript(a: string[], b: string[]): Op[] {
  let prefix = 0;
  while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix += 1;
  let suffix = 0;
  while (
    suffix < a.length - prefix &&
    suffix < b.length - prefix &&
    a[a.length - 1 - suffix] === b[b.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  const midA = a.slice(prefix, a.length - suffix);
  const midB = b.slice(prefix, b.length - suffix);
  const ops: Op[] = Array<Op>(prefix).fill("equal");
  const n = midA.length;
  const m = midB.length;
  if (n === 0 || m === 0 || (n + 1) * (m + 1) > MAX_TABLE_CELLS) {
    ops.push(...Array<Op>(n).fill("delete"), ...Array<Op>(m).fill("insert"));
  } else {
    // lcs[i][j]: Länge der längsten gemeinsamen Folge von midA[i..] und midB[j..].
    const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i -= 1) {
      for (let j = m - 1; j >= 0; j -= 1) {
        lcs[i]![j] = midA[i] === midB[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
      }
    }
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && midA[i] === midB[j]) {
        ops.push("equal");
        i += 1;
        j += 1;
      } else if (i < n && (j >= m || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) {
        ops.push("delete");
        i += 1;
      } else {
        ops.push("insert");
        j += 1;
      }
    }
  }
  ops.push(...Array<Op>(suffix).fill("equal"));
  return ops;
}

/** Vergleicht zwei Texte zeilenweise; lehnt Binärdaten und zu große Eingaben wie der Kern ab. */
export function diffBytes(left: Uint8Array, right: Uint8Array, options: DiffOptions): DiffResult {
  const l = splitLines(left, options);
  const r = splitLines(right, options);
  const ops = editScript(l.keys, r.keys);
  const hunks: DiffHunk[] = [];
  let li = 0;
  let ri = 0;
  let i = 0;
  while (i < ops.length) {
    const ls = li;
    const rs = ri;
    if (ops[i] === "equal") {
      while (i < ops.length && ops[i] === "equal") {
        li += 1;
        ri += 1;
        i += 1;
      }
      hunks.push({ kind: "equal", leftStart: ls, left: l.text.slice(ls, li), rightStart: rs, right: r.text.slice(rs, ri) });
    } else {
      while (i < ops.length && ops[i] !== "equal") {
        if (ops[i] === "delete") li += 1;
        else ri += 1;
        i += 1;
      }
      const kind = li > ls && ri > rs ? "replace" : li > ls ? "delete" : "insert";
      hunks.push({ kind, leftStart: ls, left: l.text.slice(ls, li), rightStart: rs, right: r.text.slice(rs, ri) });
    }
  }
  return {
    hunks,
    differences: hunks.filter((hunk) => hunk.kind !== "equal").length,
    leftLines: l.text.length,
    rightLines: r.text.length,
  };
}
