// Gemeinsame Hilfen für Oberflächentests: Mock-API mit von Hand gesteuertem Zeitgeber, fertig
// gebauter Speicher samt Controller und Zugriffe auf Fenster und Zeilen.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "./App";
import { createRuntime, type AppRuntime } from "./app/runtime";
import type { ControllerOptions } from "./app/controller";
import type { DualfileApi } from "./ipc/api";
import { createMockApi, type MockScheduler } from "./ipc/mock";
import type { Settings } from "./ipc/types";

export const HOME = "/home/demo";
export const DOCS = `${HOME}/Dokumente`;
export const DOWNLOADS = `${HOME}/Downloads`;

/** Zeitgeber, bei dem Aufträge der Mock-API erst auf `runAll` weiterlaufen. */
export class ManualScheduler implements MockScheduler {
  private timers = new Map<number, () => void>();
  private next = 1;
  setTimeout(callback: () => void): unknown {
    const id = this.next++;
    this.timers.set(id, callback);
    return id;
  }
  clearTimeout(handle: unknown): void {
    this.timers.delete(handle as number);
  }
  async step(): Promise<boolean> {
    const first = this.timers.entries().next();
    if (first.done) return false;
    this.timers.delete(first.value[0]);
    first.value[1]();
    await new Promise((resolve) => setTimeout(resolve, 0));
    return true;
  }
  /** Läuft, bis nichts mehr geplant ist (ein Auftrag, der auf eine Antwort wartet, bleibt stehen). */
  async runAll(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 0));
    while (await this.step()) {
      /* weiter */
    }
  }
}

export interface SetupOptions {
  language?: Settings["language"];
  left?: string;
  right?: string;
  settings?: Partial<Settings>;
  options?: ControllerOptions;
  /** Wrapper um die API, z. B. um Aufrufe zu zählen. */
  wrap?: (api: DualfileApi) => DualfileApi;
  outcome?: "ignored";
  /** Beispieldateien für den Lister (`~/Ansicht`). */
  samples?: boolean;
  /** Demo-Ordner `~/Vergleich/{links,rechts,text}` für Vergleichen/Abgleichen/Textvergleich. */
  compareDemo?: boolean;
}

const tab = (path: string) => ({ tabs: [{ path, sort: "name", filter: "" }], activeTab: 0 });

export async function setupApp(setup: SetupOptions = {}) {
  const scheduler = new ManualScheduler();
  const initialSettings: Settings = {
    version: 1, verifyPolicy: "auto",
    theme: "schiefer",
    accent: "#e0a45c",
    density: "normal",
    fontSize: 13,
    language: setup.language ?? "de",
    bookmarks: [],
    mirror: false,
    session: { windows: [tab(setup.left ?? HOME), tab(setup.right ?? DOWNLOADS)], activeWindow: 0 },
    ...setup.settings,
  };
  const base = createMockApi({ scheduler, home: HOME, now: () => Date.UTC(2026, 10, 12, 12, 0), initialSettings, samples: setup.samples, compareDemo: setup.compareDemo });
  const api = setup.wrap ? setup.wrap(base) : base;
  const runtime: AppRuntime = createRuntime(api, { saveDelayMs: 20, toastMs: 60_000, jobDismissMs: 60_000, ...setup.options });
  const user = userEvent.setup();
  const view = render(<App api={api} runtime={runtime} />);
  const helpers = makeHelpers(runtime, scheduler);
  await waitFor(() => expect(screen.getAllByRole("grid")).toHaveLength(2));
  await waitFor(() => expect(runtime.store.getState().listings[0].path).not.toBeNull());
  await waitFor(() => expect(runtime.store.getState().listings[1].path).not.toBeNull());
  return { user, api, base, scheduler, runtime, view, ...helpers };
}

function makeHelpers(runtime: AppRuntime, scheduler: ManualScheduler) {
  const lang = () => runtime.controller.language();
  const pane = (w: 0 | 1): HTMLElement =>
    screen.getByRole("region", { name: lang() === "de" ? (w === 0 ? "Linkes Fenster" : "Rechtes Fenster") : w === 0 ? "Left pane" : "Right pane" });
  /** Datenzeilen (ohne Kopfzeile) eines Fensters. */
  const rows = (w: 0 | 1): HTMLElement[] => within(pane(w)).queryAllByRole("row").slice(1);
  const names = (w: 0 | 1): string[] => rows(w).map((row) => row.querySelector(".nm")?.textContent ?? "");
  const row = (w: 0 | 1, name: string): HTMLElement => {
    const found = rows(w).find((candidate) => candidate.querySelector(".nm")?.textContent === name);
    if (!found) throw new Error(`Zeile „${name}“ fehlt in Fenster ${w}: ${names(w).join(", ")}`);
    return found;
  };
  const focused = (w: 0 | 1): string | undefined =>
    rows(w).find((candidate) => candidate.classList.contains("cur"))?.querySelector(".nm")?.textContent ?? undefined;
  const selected = (w: 0 | 1): string[] =>
    rows(w).filter((candidate) => candidate.getAttribute("aria-selected") === "true").map((r) => r.querySelector(".nm")?.textContent ?? "");
  /** Lässt alle Aufträge der Mock-API laufen. */
  const runJobs = async (): Promise<void> => {
    await act(async () => {
      await scheduler.runAll();
    });
  };
  const dialog = (): HTMLElement => screen.getByRole("dialog");
  const alertDialog = (): HTMLElement => screen.getByRole("alertdialog");
  return { pane, rows, names, row, focused, selected, runJobs, dialog, alertDialog };
}
