/** POSIX permission display; also used for browser/demo entries without a Rust DTO. */
export function formatPermissions(mode: number | null): string {
  if (mode === null) return "";
  const chars = [..."rwxrwxrwx"].map((c, i) => mode & (1 << (8 - i)) ? c : "-");
  for (const [index, bit, lower, upper] of [[2, 0o4000, "s", "S"], [5, 0o2000, "s", "S"], [8, 0o1000, "t", "T"]] as const) {
    if (mode & bit) chars[index] = chars[index] === "x" ? lower : upper;
  }
  return chars.join("");
}
