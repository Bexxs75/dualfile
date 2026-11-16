import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { setupApp } from "./test-utils";

const settingsDialog = async (app: Awaited<ReturnType<typeof setupApp>>) => {
  act(() => app.runtime.controller.openDialog({ kind: "settings" }));
  const dlg = within(await screen.findByRole("dialog", { name: "Einstellungen" }));
  await app.user.click(dlg.getByRole("tab", { name: "Info" }));
  return dlg;
};

test("Info: Fehlersuche zeigt Ort und Größe, öffnet Ordner, Issue und Logdialog", async () => {
  const logOpenFolder = vi.fn(async () => {});
  const openUrl = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, logOpenFolder, openUrl, reportIssueUrl: async () => "https://github.com/x/new" }) });
  const dlg = await settingsDialog(app);
  const section = within(dlg.getByRole("region", { name: "Fehlersuche" }));
  expect(await section.findByText("~/.local/state/dualfile/dualfile.log")).toBeInTheDocument();
  expect(section.getByText("38 KB")).toBeInTheDocument();
  await app.user.click(section.getByRole("button", { name: "Ordner öffnen" }));
  expect(logOpenFolder).toHaveBeenCalled();
  await app.user.click(section.getByRole("button", { name: "Problem auf GitHub melden" }));
  expect(openUrl).toHaveBeenCalledWith("https://github.com/x/new");
  await app.user.click(section.getByRole("button", { name: "Logdatei ansehen …" }));
  expect(await screen.findByRole("dialog", { name: "Logdatei" })).toBeInTheDocument();
});

test("Logdialog aus den Einstellungen: Schließen führt zurück zum Info-Reiter", async () => {
  const app = await setupApp();
  const dlg = await settingsDialog(app);
  await app.user.click(await dlg.findByRole("button", { name: "Logdatei ansehen …" }));
  const log = within(await screen.findByRole("dialog", { name: "Logdatei" }));
  await app.user.click(log.getByRole("button", { name: "Schließen" }));
  const back = within(await screen.findByRole("dialog", { name: "Einstellungen" }));
  expect(back.getByRole("tab", { name: "Info" })).toHaveAttribute("aria-selected", "true");
});

test("Logdialog: anonymisiert vorab, Umschalter liest ohne Anonymisierung, Kopieren und Speichern", async () => {
  const logRead = vi.fn(async (anonymize: boolean) => (anonymize ? "ERROR ~/a <server-1>" : "ERROR /home/demo/a 10.0.0.1"));
  const logSave = vi.fn(async (dir: string) => `${dir}/dualfile-log-1.txt`);
  const app = await setupApp({ wrap: api => ({ ...api, logRead, logSave }) });
  const writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  act(() => app.runtime.controller.openDialog({ kind: "log" }));
  const dlg = within(await screen.findByRole("dialog", { name: "Logdatei" }));
  expect(await dlg.findByText("<server-1>")).toBeInTheDocument();
  expect(logRead).toHaveBeenLastCalledWith(true);
  await app.user.click(dlg.getByRole("checkbox", { name: /Anonymisieren/ }));
  await waitFor(() => expect(logRead).toHaveBeenLastCalledWith(false));
  expect(await dlg.findByText(/10\.0\.0\.1/)).toBeInTheDocument();
  await app.user.click(dlg.getByRole("button", { name: "Kopieren" }));
  expect(writeText).toHaveBeenCalledWith("ERROR /home/demo/a 10.0.0.1");
  expect(dlg.getByRole("status")).toHaveTextContent("In die Zwischenablage kopiert");
  await app.user.click(dlg.getByRole("button", { name: "Im aktuellen Ordner speichern" }));
  const dir = logSave.mock.calls[0]![0];
  expect(dir.startsWith("/")).toBe(true);
  expect(logSave).toHaveBeenCalledWith(dir, false);
  expect(dlg.getByRole("status")).toHaveTextContent(`Gespeichert: ${dir}/dualfile-log-1.txt`);
});

test("Toast: nur Fehler haben den Knopf; Fehler werden geloggt, ein Fehler dabei bleibt still", async () => {
  const logFrontend = vi.fn(async () => { throw new Error("kaputt"); });
  const app = await setupApp({ wrap: api => ({ ...api, logFrontend }) });
  act(() => app.runtime.controller.toast("Hallo"));
  expect(screen.queryByRole("button", { name: "Details in der Logdatei" })).toBeNull();
  expect(logFrontend).not.toHaveBeenCalled();
  act(() => app.runtime.controller.toast(`Böse\n${"x".repeat(3000)}`, "error"));
  expect(logFrontend).toHaveBeenCalledTimes(1);
  const [level, message] = logFrontend.mock.calls[0] as unknown as [string, string];
  expect(level).toBe("error");
  expect(message).toHaveLength(2000);
  expect(message).not.toContain("\n");
  await app.user.click(screen.getByRole("button", { name: "Details in der Logdatei" }));
  expect(await screen.findByRole("dialog", { name: "Logdatei" })).toBeInTheDocument();
  expect(app.runtime.store.getState().toasts.filter(t => t.kind === "error")).toHaveLength(0);
});
