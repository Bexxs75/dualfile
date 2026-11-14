import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { formatBytes } from "./i18n";
import { HOME, setupApp } from "./test-utils";

for (const language of ["de", "en"] as const) {
  test(`free space is localized and refreshes after a directory change (${language})`, async () => {
    const spaceFree = vi.fn(async () => ({ available: 123_400_000_000, total: 200_000_000_000 }));
    const app = await setupApp({ language, wrap: api => ({ ...api, spaceFree }) });
    const status = within(app.pane(0)).getByRole("status");
    await waitFor(() => expect(status).toHaveTextContent(`${formatBytes(123_400_000_000, language)} ${language === "de" ? "frei" : "free"}`));
    expect(status.querySelector(".status-context")).toHaveClass("tabular-nums");
    const calls = spaceFree.mock.calls.length;
    await act(async () => { await app.runtime.controller.reload(0); });
    await waitFor(() => expect(spaceFree.mock.calls.length).toBeGreaterThan(calls));
    spaceFree.mockResolvedValue({ available: 0, total: 200_000_000_000 });
    await act(async () => { await app.runtime.controller.reload(0); });
    await waitFor(() => expect(status).toHaveTextContent(language === "de" ? "0 B frei" : "0 B free"));
  });

  test(`connection loss reaches item and completed job dialogs (${language})`, async () => {
    const app = await setupApp({ language });
    act(() => app.runtime.store.dispatch({ type: "itemErrorAdd", error: {
      jobId: 42, name: "file.txt", kind: "disconnected", message: "raw transport detail",
    } }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(language === "de" ? "Die Verbindung zum Server ist abgebrochen." : "The connection to the server was lost.");
    expect(within(dialog).getByRole("button", { name: language === "de" ? "Überspringen" : "Skip" })).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: language === "de" ? "Wiederholen" : "Retry" })).toBeEnabled();
    expect(dialog).not.toHaveTextContent("raw transport detail");
    act(() => {
      app.runtime.store.dispatch({ type: "itemErrorDone", jobId: 42 });
      app.runtime.store.dispatch({ type: "job", event: { type: "job", status: {
        id: 42, name: "file.txt", state: "failed", filesDone: 0, bytesDone: 0,
        bytesTotal: 1, bytesPerSecond: 0, etaSeconds: null, queuePosition: null, lane: null,
        failed: [{ path: "file.txt", kind: "disconnected", message: "raw transport detail" }], skipped: [], warnings: [],
      } } });
    });
    expect(screen.getByText(language === "de" ? /Grund:.*Verbindung zum Server/ : /Reason:.*connection to the server/)).toBeVisible();
  });
}

test.each([HOME, "sftp:test!/"])("null space is silent, with remote fallback: %s", async path => {
  const spaceFree = vi.fn(async () => null);
  const app = await setupApp({ left: path, wrap: api => ({ ...api, spaceFree }) });
  await waitFor(() => expect(spaceFree).toHaveBeenCalledWith(path));
  const status = within(app.pane(0)).getByRole("status");
  expect(status).not.toHaveTextContent(/frei/);
  if (path !== HOME) expect(status).toHaveTextContent("Fernzugriff");
  else expect(status).not.toHaveTextContent("Fernzugriff");
});


test("a completed copy refreshes destination space; rejected probes stay silent", async () => {
  let available = 1000;
  const spaceFree = vi.fn(async () => ({ available, total: 2000 }));
  const app = await setupApp({ wrap: api => ({ ...api, spaceFree }) });
  const status = within(app.pane(1)).getByRole("status");
  await waitFor(() => expect(status).toHaveTextContent(`${formatBytes(1000, "de")} frei`));
  available = 500;
  await act(async () => { await app.api.copy([`${HOME}/Dokumente/notizen.txt`], `${HOME}/Downloads`); });
  await app.runJobs();
  await waitFor(() => expect(status).toHaveTextContent("500 B frei"));
  spaceFree.mockRejectedValue(new Error("private transport details"));
  await act(async () => { await app.runtime.controller.reload(1); });
  await waitFor(() => expect(status).not.toHaveTextContent("frei"));
  expect(screen.queryByText("private transport details")).not.toBeInTheDocument();
});
