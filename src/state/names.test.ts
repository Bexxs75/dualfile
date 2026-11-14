import { describe, expect, test } from "vitest";
import { nameError } from "./names";

describe("nameError", () => {
  test("erlaubt gewöhnliche Namen", () => {
    expect(nameError("bericht.txt", ["a", "b"])).toBeNull();
    expect(nameError("Ünïcode ✓", [])).toBeNull();
  });
  test.each([
    ["", "error.name.empty"],
    ["   ", "error.name.empty"],
    [".", "error.name.dot"],
    ["..", "error.name.dot"],
    ["a/b", "error.name.slash"],
    ["a\0b", "error.name.slash"],
    ["x".repeat(256), "error.name.tooLong"],
  ])("lehnt %j ab", (name, key) => {
    expect(nameError(name, [])).toBe(key);
  });
  test("255 Bytes sind erlaubt, 256 nicht (Bytes, nicht Zeichen)", () => {
    expect(nameError("x".repeat(255), [])).toBeNull();
    expect(nameError("ä".repeat(128), [])).toBe("error.name.tooLong");
  });
  test("vorhandener Name wird gemeldet, der eigene bisherige nicht", () => {
    expect(nameError("b", ["a", "b"])).toBe("error.name.exists");
    expect(nameError("b", ["a", "b"], "b")).toBeNull();
  });
});
