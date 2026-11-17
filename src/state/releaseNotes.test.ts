import { expect, test } from "vitest";
import config from "../../src-tauri/tauri.conf.json";
import { releaseNotes } from "../release-notes";
import { compareVersions, seenVersionOnClose, selectReleaseVersions } from "./releaseNotes";

const versions = ["0.3.3", "0.3.4", "0.3.5", "0.3.6"];
const loaded = { kind: "loaded" } as const;
test("fresh and newer-version settings suppress automatic notes", () => {
  expect(selectReleaseVersions(versions, "0.3.5", null, { kind: "fresh" })).toEqual([]);
  expect(selectReleaseVersions(versions, "0.3.5", null, { kind: "newerVersion", version: 99 })).toEqual([]);
});
test("legacy settings show only current; skipped versions are newest first and bounded", () => {
  expect(selectReleaseVersions(versions, "0.3.5", undefined, loaded)).toEqual(["0.3.5"]);
  expect(selectReleaseVersions(versions, "0.3.5", "0.3.2", loaded)).toEqual(["0.3.5", "0.3.4", "0.3.3"]);
  expect(selectReleaseVersions(versions, "0.3.5", "0.3.5", loaded)).toEqual([]);
  expect(selectReleaseVersions(versions, "0.3.5", "0.3.6", loaded)).toEqual([]);
});
test("real semver orders numeric components, prereleases and ignores build metadata", () => {
  const ordered = ["0.3.9", "0.3.10-alpha", "0.3.10-alpha.1", "0.3.10-alpha.2", "0.3.10-alpha.10", "0.3.10-beta", "0.3.10-rc.1", "0.3.10", "1.0.0"];
  for (let i = 1; i < ordered.length; i++) expect(compareVersions(ordered[i - 1]!, ordered[i]!)).toBeLessThan(0);
  expect(compareVersions("0.3.5+one", "0.3.5+two")).toBe(0);
  for (const invalid of ["01.2.3", "1.2", "1.2.3-01", "1.2.3\n"]) expect(() => compareVersions(invalid, "1.2.3")).toThrow();
});
test("WN-01: manual close on 0.3.5 preserves 0.3.6 and returning to 0.3.6 stays quiet", () => {
  const marker = seenVersionOnClose("0.3.5", "0.3.6");
  expect(marker).toBe("0.3.6");
  expect(selectReleaseVersions(versions, "0.3.6", marker, loaded)).toEqual([]);
  expect(seenVersionOnClose("0.3.5", null)).toBe("0.3.5");
  expect(seenVersionOnClose("0.3.5", "0.3.4")).toBe("0.3.5");
});
test("release guard: the packaged version has nonempty German and English release notes", () => {
  expect(releaseNotes[config.version]).toBeDefined();
  for (const note of Object.values(releaseNotes)) {
    expect(note.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    for (const language of ["de", "en"] as const) {
      const text = Object.values(note[language]).flat();
      expect(text.length).toBeGreaterThan(0);
      expect(text.every(line => line.trim().length > 0)).toBe(true);
    }
  }
});
