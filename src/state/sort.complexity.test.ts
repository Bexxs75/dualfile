// Issue #63 / #28: sortEntries must stay O(n log n). Counts comparator calls instead of measuring
// time, so the check is deterministic on shared CI runners.
import { afterEach, expect, test, vi } from "vitest";
import type { Entry } from "../ipc/types";
import { sortEntries, type SortKey } from "./sort";

const N = 50_000;
/** Generous constant: TimSort needs about n·log2(n) comparisons on random input, fewer on runs. */
const C = 2;

afterEach(() => vi.restoreAllMocks());

function entries(): Entry[] {
  const out: Entry[] = [];
  // Deterministic pseudo-random order (LCG), mixed files and folders, names with numbers.
  let seed = 12345;
  const next = () => (seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648);
  for (let i = 0; i < N; i++) {
    const r = next();
    const dir = r % 5 === 0;
    out.push({ name: dir ? `Ordner ${r % 9973}` + `_${i}` : `Datei_${r % 100_003}_${i}.${["txt", "pdf", "jpg"][r % 3]}`, kind: dir ? "dir" : "file", size: r % 100_000, modified: 1_500_000_000_000 + (r % 300_000_000) * 1000, mode: 0o644, uid: 1000, gid: 1000, ownerName: "tester", groupName: "tester", permissions: "rw-r--r--", hidden: false, linkTarget: null, brokenLink: false, error: null });
  }
  return out;
}

function countComparisons(run: () => void): number {
  let calls = 0;
  const original = Array.prototype.sort;
  vi.spyOn(Array.prototype, "sort").mockImplementation(function (this: unknown[], compare?: (a: unknown, b: unknown) => number) {
    return original.call(this, compare ? (a, b) => { calls++; return compare(a, b); } : undefined);
  });
  run();
  return calls;
}

test.each<SortKey>(["name", "size", "type", "modified"])("sortEntries by %s needs at most c·n·log2(n) comparisons", (key) => {
  const input = entries();
  const bound = C * N * Math.log2(N);
  for (const direction of ["asc", "desc"] as const) {
    const calls = countComparisons(() => { sortEntries(input, key, direction); });
    expect(calls).toBeGreaterThan(N - 1); // the counter really saw the sort
    expect(calls).toBeLessThanOrEqual(bound);
    vi.restoreAllMocks();
  }
});
