import { expect, test } from "vitest";
import { createTranslator } from "../i18n";
import type { JobStatus } from "../ipc/types";
import { formatJobPresentation } from "./jobPresentation";
const job: JobStatus = { id: 1, name: "test", state: "running", filesDone: 0, bytesDone: 0, bytesTotal: 0, bytesPerSecond: 0, etaSeconds: null, queuePosition: null, lane: null, failed: [], skipped: [], warnings: [] };
const tr = createTranslator("en");
test("unknown totals, directories and scan snapshots never invent totals", () => {
  for (const filesDone of [0, 6, 4951]) for (const state of ["running", "done"] as const) {
    expect(formatJobPresentation({ job: { ...job, filesDone, state } }, tr).counter).toBe(`${filesDone} items`);
  }
});
test("partial copy completion and undo describe outcomes", () => {
  const result = formatJobPresentation({ job: { ...job, state: "done", filesDone: 1, skipped: ["a", "b", "c", "d", "e"] }, kind: "copy" }, tr);
  expect(result.counter).toBe("1 copied, 5 skipped");
  expect(result.state).toBe("Done with skips");
  expect(formatJobPresentation({ job: { ...job, state: "done", filesDone: 1 }, kind: "undo" }, tr).counter).toBe("Undo: 1");
  expect(formatJobPresentation({ undo: { done: 1, remaining: 0 } }, tr).counter).toBe("1 undone");
  expect(formatJobPresentation({ undo: { done: 1, remaining: 2 } }, tr).counter).toBe("1 of 3 undone, 2 remaining");
});
test("ETA has short seconds and hides below five seconds, finished and waiting jobs", () => {
  for (const [etaSeconds, expected] of [[2, null], [4, null], [5, "about 5 s remaining"], [59, "about 59 s remaining"], [62, "about 1 min 2 s remaining"]] as const) {
    expect(formatJobPresentation({ job: { ...job, etaSeconds } }, tr).eta).toBe(expected);
  }
  expect(formatJobPresentation({ job: { ...job, etaSeconds: 60, state: "done" } }, tr).eta).toBeNull();
  const waiting = formatJobPresentation({ job: { ...job, etaSeconds: 60 }, waiting: true }, tr);
  expect(waiting.state).toBe("Waiting for input");
  expect(waiting.eta).toBeNull();
});
test("German outcomes are translated", () => {
  expect(formatJobPresentation({ undo: { done: 1, remaining: 0 } }, createTranslator("de")).counter).toBe("1 rückgängig gemacht");
});
