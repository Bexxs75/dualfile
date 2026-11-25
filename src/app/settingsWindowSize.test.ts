import { describe, expect, test } from "vitest";
import { answerPatch, decideSize, NO_SIZE, SIZE_THRESHOLD } from "./settingsWindowSize";

const standard = { width: 720, height: 640 };

describe("decideSize", () => {
  test("the threshold is 8 px: eight is nothing, nine is a change (either side, either direction)", () => {
    expect(SIZE_THRESHOLD).toBe(8);
    for (const mode of ["ask", "always"] as const) {
      expect(decideSize(mode, standard, standard)).toEqual({ kind: "none" });
      expect(decideSize(mode, { width: 728, height: 648 }, standard)).toEqual({ kind: "none" });
      expect(decideSize(mode, { width: 712, height: 632 }, standard)).toEqual({ kind: "none" });
      expect(decideSize(mode, { width: 729, height: 640 }, standard).kind).not.toBe("none");
      expect(decideSize(mode, { width: 720, height: 631 }, standard).kind).not.toBe("none");
    }
  });
  test("never stores nothing and never asks, however large the change", () => {
    expect(decideSize("never", { width: 1600, height: 900 }, standard)).toEqual({ kind: "none" });
  });
  test("always saves without asking", () => {
    expect(decideSize("always", { width: 900, height: 700 }, standard)).toEqual({ kind: "save", size: { width: 900, height: 700 } });
  });
  test("ask asks and names the direction by area; an unset mode asks", () => {
    expect(decideSize("ask", { width: 900, height: 700 }, standard)).toEqual({ kind: "ask", size: { width: 900, height: 700 }, direction: "larger" });
    expect(decideSize("ask", { width: 600, height: 500 }, standard)).toEqual({ kind: "ask", size: { width: 600, height: 500 }, direction: "smaller" });
    expect(decideSize(undefined, { width: 600, height: 500 }, standard).kind).toBe("ask");
  });
  test("the reference is the opened size, so a remembered size that did not change is not asked about again", () => {
    const remembered = { width: 900, height: 700 };
    expect(decideSize("ask", remembered, remembered)).toEqual({ kind: "none" });
    expect(decideSize("ask", standard, remembered)).toMatchObject({ kind: "ask", direction: "smaller" });
  });
  test("fractional sizes are rounded", () => {
    expect(decideSize("always", { width: 900.4, height: 700.6 }, standard)).toEqual({ kind: "save", size: { width: 900, height: 701 } });
  });
});

describe("answerPatch", () => {
  const size = { width: 900, height: 700 };
  test("yes stores the size, no forgets it, without touching the mode", () => {
    expect(answerPatch({ choice: "yes", dontAsk: false }, size)).toEqual({ settingsWindowSize: size });
    expect(answerPatch({ choice: "no", dontAsk: false }, size)).toEqual({ settingsWindowSize: NO_SIZE });
  });
  test("with the tick the mode follows the answer", () => {
    expect(answerPatch({ choice: "yes", dontAsk: true }, size)).toEqual({ settingsWindowSize: size, settingsWindowSizeMode: "always" });
    expect(answerPatch({ choice: "no", dontAsk: true }, size)).toEqual({ settingsWindowSize: NO_SIZE, settingsWindowSizeMode: "never" });
  });
  test("Esc changes nothing, whatever the tick says", () => {
    expect(answerPatch({ choice: "dismiss", dontAsk: false }, size)).toBeNull();
    expect(answerPatch({ choice: "dismiss", dontAsk: true }, size)).toBeNull();
  });
});
