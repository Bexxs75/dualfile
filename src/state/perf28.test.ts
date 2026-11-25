// Load measurement for issue #28 (sort and filter of 50,000 entries). Runs only with
// DUALFILE_PERF=1 so the normal test run stays fast and free of timing:
//   DUALFILE_PERF=1 npx vitest run src/state/perf28.test.ts
// This is the pure-JS part (no DOM, no rendering). It runs in Node/V8, not in the WebKitGTK
// engine the app uses, so the numbers are a lower bound / proxy, not a UI measurement.
import { describe, it } from "vitest";
import type { Entry } from "../ipc/types";
import { filterEntries } from "./filter";
import { sortEntries, type SortKey } from "./sort";

const N = 50_000;
// `process` is not in the DOM-only tsconfig types; read the switch without a Node type dependency.
const enabled = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env.DUALFILE_PERF === "1";

function fileName(i: number): string {
  switch (i % 8) {
    case 0: return `IMG_${String(i).padStart(5, "0")}.jpg`;
    case 1: return `Bericht_2026-${String((i % 12) + 1).padStart(2, "0")}_${i}.pdf`;
    case 2: return `Müller_Rechnung_${i}.docx`;
    case 3: return `日本語_${i}.txt`;
    case 4: return `résumé_${i}.md`;
    case 5: return `Datei ${i} (Kopie).txt`;
    case 6: return `scan-${i}.PNG`;
    default: return `.verborgen_${i}`;
  }
}

function dirName(i: number): string {
  return ["Projekt_", "Übersicht ", "ordner-", "Ärger_"][i % 4] + i;
}

function make(): Entry[] {
  const out: Entry[] = [];
  const files = (N * 6) / 10;
  for (let i = 0; i < files; i++) {
    const name = fileName(i);
    out.push({ name, kind: "file", size: (i * 7919) % 100_000, modified: 1_500_000_000_000 + ((i * 104_729) % 300_000_000) * 1000, mode: 0o644, uid: 1000, gid: 1000, ownerName: "tester", groupName: "tester", permissions: "rw-r--r--", hidden: name.startsWith("."), linkTarget: null, brokenLink: false, error: null });
  }
  for (let i = 0; i < N - files; i++) {
    out.push({ name: dirName(i), kind: "dir", size: 0, modified: 1_600_000_000_000 + i * 1000, mode: 0o755, uid: 1000, gid: 1000, ownerName: "tester", groupName: "tester", permissions: "rwxr-xr-x", hidden: false, linkTarget: null, brokenLink: false, error: null });
  }
  // The directory listing arrives in file-system order, not sorted: shuffle deterministically.
  let seed = 42;
  for (let i = out.length - 1; i > 0; i--) {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    const j = seed % (i + 1);
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

function runs(label: string, fn: () => unknown): void {
  const times: number[] = [];
  for (let r = 0; r < 5; r++) {
    const t = performance.now();
    fn();
    times.push(performance.now() - t);
  }
  const s = [...times].sort((a, b) => a - b);
  console.log(`${label.padEnd(44)} median ${s[2].toFixed(1).padStart(7)} ms  min ${s[0].toFixed(1).padStart(7)}  max ${s[4].toFixed(1).padStart(7)}  (n=5, first run ${times[0].toFixed(1)})`);
}

describe.skipIf(!enabled)("perf #28 sort/filter, 50k entries", () => {
  it("measures", () => {
    const entries = make();
    console.log(`\n${entries.length} entries`);
    const keys: SortKey[] = ["name", "size", "type", "modified"];
    for (const key of keys) runs(`sortEntries key=${key} asc`, () => sortEntries(entries, key, "asc"));
    runs("sortEntries key=name desc", () => sortEntries(entries, "name", "desc"));
    runs("filterEntries '' (hidden off)", () => filterEntries(entries, "", false));
    runs("filterEntries 'bericht' (substring)", () => filterEntries(entries, "bericht", false));
    runs("filterEntries '*.pdf' (wildcard)", () => filterEntries(entries, "*.pdf", false));
    runs("filter 'bericht' + sort name (visibleEntries)", () => sortEntries(filterEntries(entries, "bericht", false), "name", "asc"));
    runs("filter '' + sort name (full pipeline)", () => sortEntries(filterEntries(entries, "", false), "name", "asc"));
  });
});
