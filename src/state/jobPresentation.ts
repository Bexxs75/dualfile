import type { JobKind } from "../app/state";
import type { JobStatus } from "../ipc/types";
import type { TranslationKey, TranslationParams } from "../i18n";
import { isFinished } from "./operations";

type Input = { job: JobStatus; kind?: JobKind; waiting?: boolean } | { undo: { done: number; remaining: number } };
/** Core counts completed entries (including directories) and supplies no stable item total.
 * Scan snapshots may lag behind the result, so none of these counters claim a total. */
export function formatJobPresentation(input: Input, tr: (key: TranslationKey, params?: TranslationParams) => string): { counter: string; state: string; eta: string | null } {
  if ("undo" in input) {
    const { done, remaining } = input.undo;
    return { counter: tr(remaining === 0 ? "undo.complete" : "undo.summary", { done, remaining, total: done + remaining }), state: "", eta: null };
  }
  const { job, kind, waiting = false } = input;
  const finished = isFinished(job.state);
  const needsInput = !finished && waiting;
  let counter = tr(kind === "undo" ? "jobs.undo" : finished && kind === "copy" ? "jobs.copied" : "jobs.items", { done: job.filesDone });
  if (job.skipped.length > 0) counter = tr("jobs.outcome", { done: counter, skipped: job.skipped.length });
  const state = tr(needsInput ? "jobs.waitingInput" : job.state === "done" && job.skipped.length > 0 ? "jobs.doneSkipped" : job.state === "done" && job.verified ? "jobs.state.verified" : `jobs.state.${job.state}`);
  const seconds = job.etaSeconds;
  const eta = job.state === "running" && !needsInput && seconds !== null && seconds >= 5
    ? tr(seconds < 60 ? "jobs.etaSeconds" : "jobs.eta", { minutes: Math.floor(seconds / 60), seconds: seconds % 60 }) : null;
  return { counter, state, eta };
}
