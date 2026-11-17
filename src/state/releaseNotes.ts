import type { LoadOutcome } from "../ipc/types";

const pattern = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
function parse(version: string) {
  const match = pattern.exec(version);
  if (!match || match[0] !== version) throw new Error(`Invalid semantic version: ${version}`);
  return { core: match.slice(1, 4).map(v => BigInt(v!)), pre: match[4]?.split(".") };
}
/** SemVer 2.0 precedence, including prereleases; build metadata has no precedence. */
export function compareVersions(a: string, b: string): number {
  const left = parse(a), right = parse(b);
  for (let i = 0; i < 3; i++) {
    if (left.core[i] !== right.core[i]) return left.core[i]! > right.core[i]! ? 1 : -1;
  }
  if (!left.pre || !right.pre) return left.pre ? -1 : right.pre ? 1 : 0;
  for (let i = 0; i < Math.max(left.pre.length, right.pre.length); i++) {
    const x = left.pre[i], y = right.pre[i];
    if (x === y) continue;
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    const xn = /^\d+$/.test(x), yn = /^\d+$/.test(y);
    if (xn && yn) return BigInt(x) > BigInt(y) ? 1 : -1;
    if (xn !== yn) return xn ? -1 : 1;
    return x > y ? 1 : -1;
  }
  return 0;
}
export function selectReleaseVersions(versions: readonly string[], current: string, lastSeen: string | null | undefined, outcome: LoadOutcome): string[] {
  if (outcome.kind === "fresh" || outcome.kind === "newerVersion") return [];
  return versions.filter(v => compareVersions(v, current) <= 0 &&
    (lastSeen == null ? compareVersions(v, current) === 0 : compareVersions(v, lastSeen) > 0))
    .sort((a, b) => compareVersions(b, a));
}
export function seenVersionOnClose(current: string, stored: string | null | undefined): string {
  return stored == null || compareVersions(current, stored) > 0 ? current : stored;
}
