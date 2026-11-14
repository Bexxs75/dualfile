import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime, type AppRuntime } from "../app/runtime";
import { defaultSettings } from "../app/state";
import { createMockApi } from "../ipc/mock";
import type { JobStatus } from "../ipc/types";
import { ManualScheduler } from "../test-utils";
import { JobsPanel } from "./JobsPanel";
import { JobDialogs } from "./dialogs/JobDialogs";
import { SettingsDialog } from "./dialogs/SettingsDialog";

const runtimes: AppRuntime[] = [];
afterEach(() => runtimes.splice(0).forEach(r => r.controller.dispose()));
async function setup(itemError = false) {
  const scheduler = new ManualScheduler();
  const api = createMockApi({ scheduler, itemError, initialSettings: { ...defaultSettings, language: "de" } });
  const rt = createRuntime(api, { jobDismissMs: 60000, saveDelayMs: 1 });
  runtimes.push(rt);
  await rt.controller.start();
  const view = render(<RuntimeProvider runtime={rt}><JobsPanel /><JobDialogs /></RuntimeProvider>);
  return { api, rt, scheduler, view, user: userEvent.setup() };
}
const status = (patch: Partial<JobStatus> = {}): JobStatus => ({
  id: 1, name: "a.txt", state: "running", filesDone: 2, bytesDone: 50, bytesTotal: 100,
  bytesPerSecond: 10, etaSeconds: 68, queuePosition: null, lane: null, failed: [], skipped: [], warnings: [], ...patch,
});

test("collapse persists in session; progress, ETA, queue and state announcements", async () => {
  const { rt, user } = await setup();
  act(() => rt.store.dispatch({ type: "job", event: { type: "job", status: status() } }));
  expect(screen.getByText("noch ca. 1 Min. 8 s")).toBeVisible();
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuemin", "0");
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuemax", "100");
  expect(document.querySelector(".job-state")).toHaveAttribute("aria-live", "polite");
  await user.click(screen.getByRole("button", { name: "Aufträge einklappen" }));
  expect(screen.queryByRole("progressbar")).toBeNull();
  act(() => rt.store.dispatch({ type: "job", event: { type: "job", status: status({ state: "queued", queuePosition: 2 }) } }));
  expect(screen.queryByRole("progressbar")).toBeNull();
  await user.click(screen.getByRole("button", { name: /Aufträge · 1 aktiv/ }));
  expect(screen.getByText("wartet, Platz 2")).toBeVisible();
  act(() => rt.store.dispatch({ type: "job", event: { type: "job", status: status({ state: "done", verified: true }) } }));
  expect(screen.getByText("fertig, geprüft")).toBeVisible();
});

test("pause, resume and pause/resume all call the API", async () => {
  const { rt, api, user } = await setup();
  const pause = vi.spyOn(api, "jobPause").mockResolvedValue();
  const resume = vi.spyOn(api, "jobResume").mockResolvedValue();
  const put = (state: JobStatus["state"]) => act(() => {
    for (const id of [1, 2]) rt.store.dispatch({ type: "job", event: { type: "job", status: status({ id, state }) } });
  });
  put("running");
  await user.click(screen.getAllByRole("button", { name: "Pausieren" })[0]!);
  expect(pause).toHaveBeenCalledWith(1);
  await user.click(screen.getByRole("button", { name: "Alle pausieren" }));
  expect(pause).toHaveBeenCalledWith(2);
  put("paused");
  await user.click(screen.getAllByRole("button", { name: "Fortsetzen" })[0]!);
  expect(resume).toHaveBeenCalledWith(1);
  await user.click(screen.getByRole("button", { name: "Alle fortsetzen" }));
  expect(resume).toHaveBeenCalledWith(2);
});

test("checksum setting persists through the settings API", async () => {
  const { rt, api, view, user } = await setup();
  const save = vi.spyOn(api, "saveSettings");
  view.rerender(<RuntimeProvider runtime={rt}><SettingsDialog /></RuntimeProvider>);
  await user.selectOptions(screen.getByLabelText("Nach dem Kopieren vergleichen (SHA-256)"), "always");
  await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ verifyPolicy: "always" })));
});

test("mock close request focuses keep-working and confirmation cancels active jobs", async () => {
  const { api, scheduler, user } = await setup();
  const confirm = vi.spyOn(api, "appConfirmClose");
  let id = 0;
  await act(async () => { id = await api.copy(["/home/demo/Dokumente/notizen.txt"], "/home/demo/Downloads"); });
  act(() => api.requestClose());
  expect(screen.getByRole("button", { name: "Weiterarbeiten" })).toHaveFocus();
  await user.click(screen.getByRole("button", { name: "Weiterarbeiten" }));
  expect(confirm).not.toHaveBeenCalled();
  act(() => api.requestClose());
  await user.click(screen.getByRole("button", { name: "Abbrechen und beenden" }));
  await act(async () => { await scheduler.runAll(); });
  expect(confirm).toHaveBeenCalledTimes(1);
  expect((await api.jobs()).find(j => j.id === id)?.state).toBe("cancelled");
  act(() => api.requestClose());
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

for (const [decision, text] of [["skip", "Überspringen"], ["retry", "Wiederholen"], ["abort", "Abbrechen"]] as const) {
  test(`item error mock flow: ${decision}`, async () => {
    const { api, scheduler, user } = await setup(true);
    const answer = vi.spyOn(api, "answerItemError");
    let id = 0;
    await act(async () => { id = await api.copy(["/home/demo/Dokumente/notizen.txt"], "/home/demo/Downloads"); await scheduler.runAll(); });
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("notizen.txt")).toBeVisible();
    expect(dialog.textContent).not.toContain("/home/demo");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: text }));
    await act(async () => { await scheduler.runAll(); });
    expect(answer).toHaveBeenCalledWith(id, decision);
    const job = (await api.jobs()).find(j => j.id === id)!;
    expect(job.state).toBe(decision === "abort" ? "cancelled" : decision === "retry" ? "done" : "failed");
  });
}

test("remember skips subsequent errors in this job, but asks again in another job", async () => {
  const { api, scheduler, user } = await setup(true);
  const answer = vi.spyOn(api, "answerItemError");
  let id = 0;
  await act(async () => {
    id = await api.copy(["/home/demo/Dokumente/notizen.txt", "/home/demo/Dokumente/Änderungen.txt"], "/home/demo/Downloads");
    await scheduler.runAll();
  });
  await user.click(screen.getByRole("checkbox"));
  await user.click(screen.getByRole("button", { name: "Überspringen" }));
  await act(async () => { await scheduler.runAll(); });
  expect(answer.mock.calls).toEqual([[id, "skip"], [id, "skip"]]);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect((await api.jobs()).find(job => job.id === id)?.failed).toHaveLength(2);
  await act(async () => {
    await api.copy(["/home/demo/Dokumente/notizen.txt"], "/home/demo/Downloads");
    await scheduler.runAll();
  });
  expect(screen.getByRole("dialog")).toBeVisible();
  expect(screen.getByRole("checkbox")).not.toBeChecked();
  expect(answer).toHaveBeenCalledTimes(2);
});

test("checked remember does not remember retry", async () => {
  const { api, scheduler, user } = await setup(true);
  const answer = vi.spyOn(api, "answerItemError");
  await act(async () => {
    await api.copy(["/home/demo/Dokumente/notizen.txt", "/home/demo/Dokumente/Änderungen.txt"], "/home/demo/Downloads");
    await scheduler.runAll();
  });
  await user.click(screen.getByRole("checkbox"));
  await user.click(screen.getByRole("button", { name: "Wiederholen" }));
  await act(async () => { await scheduler.runAll(); });
  expect(within(screen.getByRole("dialog")).getByText("Änderungen.txt")).toBeVisible();
  expect(answer).toHaveBeenCalledTimes(1);
});

test("failed error answer keeps the question available for another attempt", async () => {
  const { rt, api, user } = await setup();
  vi.spyOn(api, "answerItemError").mockRejectedValueOnce(new Error("offline")).mockResolvedValue();
  act(() => rt.store.dispatch({ type: "itemErrorAdd", error: { jobId: 7, name: "x", kind: "permission", message: "" } }));
  await user.click(screen.getByRole("button", { name: "Überspringen" }));
  expect(screen.getByRole("dialog")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Überspringen" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("all state badges and fixed checksum failure text are displayed", async () => {
  const { rt } = await setup();
  act(() => {
    for (const [id, state] of (["queued", "running", "paused", "done", "failed", "cancelled"] as const).entries()) {
      rt.store.dispatch({ type: "job", event: { type: "job", status: status({ id, state,
        failed: state === "failed" ? [{ path: "x", kind: "verifyFailed", message: "backend detail" }] : [],
      }) } });
    }
  });
  for (const [state, label] of Object.entries({ queued: "Wartet", running: "Läuft", paused: "Pausiert", done: "Fertig", failed: "Mit Fehlern beendet", cancelled: "Abgebrochen" })) {
    expect(document.querySelector(`[data-state="${state}"] .job-state`)).toHaveTextContent(label);
  }
  expect(screen.getByText(/Die Prüfsumme stimmt nach dem Übertragen nicht überein/)).toBeVisible();
  expect(screen.queryByText(/backend detail/)).toBeNull();
});

test("job cards use honest entry counts, short ETA, skipped outcomes and waiting state", async () => {
  const { rt } = await setup();
  const put = (patch: Partial<JobStatus>) => act(() => rt.store.dispatch({ type: "job", event: { type: "job", status: status(patch) } }));
  put({ filesDone: 0, etaSeconds: 2 });
  expect(screen.getByText("0 Elemente")).toBeVisible();
  expect(screen.queryByText(/0 Min/)).toBeNull();
  put({ filesDone: 6, state: "done" });
  expect(screen.getByText("6 Elemente")).toBeVisible();
  act(() => rt.store.dispatch({ type: "jobKind", id: 2, kind: "copy" }));
  put({ id: 2, state: "done", filesDone: 1, skipped: ["a", "b", "c", "d", "e"] });
  expect(screen.getByText("1 kopiert, 5 übersprungen")).toBeVisible();
  expect(screen.getByText("Fertig mit übersprungenen Elementen")).toBeVisible();
  put({ id: 3, etaSeconds: 12 });
  expect(screen.getByText("noch ca. 12 s")).toBeVisible();
  act(() => rt.store.dispatch({ type: "itemErrorAdd", error: { jobId: 3, name: "x", kind: "permission", message: "" } }));
  expect(screen.getByText("Wartet auf Eingabe")).toBeVisible();
  expect(screen.queryByText("noch ca. 12 s")).toBeNull();
});
