import type { SettingsWindowPatch, SettingsWindowSizeMode, WindowSize } from "../ipc/types";

/** Smaller changes (a few pixels from grabbing the edge) are not worth a question. */
export const SIZE_THRESHOLD = 8;
export const NO_SIZE: WindowSize = { width: 0, height: 0 };

export const sizeDiffers = (a: WindowSize, b: WindowSize) => Math.abs(a.width - b.width) > SIZE_THRESHOLD || Math.abs(a.height - b.height) > SIZE_THRESHOLD;

export type SizeDecision = { kind: "none" } | { kind: "save"; size: WindowSize } | { kind: "ask"; size: WindowSize; direction: "larger" | "smaller" };
export type SizeAnswer = { choice: "yes" | "no" | "dismiss"; dontAsk: boolean };

/**
 * What closing the settings window has to do about its size. `reference` is the size the window
 * opened with (as measured when it settled: the remembered size, or the standard size). Only a change of more than
 * `SIZE_THRESHOLD` pixels counts; "never" stores nothing and never asks, "always" stores
 * silently, "ask" asks.
 */
export function decideSize(mode: SettingsWindowSizeMode | undefined, current: WindowSize, reference: WindowSize): SizeDecision {
  if (mode === "never" || !sizeDiffers(current, reference)) return { kind: "none" };
  const size = { width: Math.round(current.width), height: Math.round(current.height) };
  if (mode === "always") return { kind: "save", size };
  return { kind: "ask", size, direction: size.width * size.height >= reference.width * reference.height ? "larger" : "smaller" };
}

/**
 * Patch for the answer: yes stores the size, no forgets it, with "don't ask again" the mode
 * follows ("always" after yes, "never" after no). Esc ("dismiss") changes nothing and never
 * touches the mode.
 */
export function answerPatch(answer: SizeAnswer, size: WindowSize): SettingsWindowPatch | null {
  if (answer.choice === "dismiss") return null;
  const yes = answer.choice === "yes";
  return { settingsWindowSize: yes ? size : NO_SIZE, ...(answer.dontAsk ? { settingsWindowSizeMode: yes ? "always" : "never" } as const : {}) };
}
