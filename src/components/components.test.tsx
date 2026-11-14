import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime } from "../app/runtime";
import { createMockApi } from "../ipc/mock";
import type { ConflictInfo, JobStatus } from "../ipc/types";
import { ConflictDialog } from "./dialogs/ConflictDialog";
import { DeleteDialog } from "./dialogs/DeleteDialog";
import { deleteDialogModel } from "../state/operations";
import { JobsPanel } from "./JobsPanel";
import { useGlobalKeys } from "../app/keyboard";
import { Modal } from "./Modal";
import { Toasts } from "./Toasts";

async function runtime(language: "de" | "en" = "de") {
  const api = createMockApi({
    initialSettings: {
      version: 1, verifyPolicy: "auto", theme: "schiefer", accent: "#e0a45c", density: "normal", fontSize: 13, language,
      bookmarks: [], mirror: false, session: { windows: [], activeWindow: 0 },
    },
  });
  const rt = createRuntime(api, { toastMs: 50 });
  await rt.controller.start();
  return { api, rt };
}
const withRuntime = (rt: Awaited<ReturnType<typeof runtime>>["rt"], node: React.ReactNode) => (
  <RuntimeProvider runtime={rt}>{node}</RuntimeProvider>
);

describe("Modal", () => {
  function Host({ onClose }: { onClose: () => void }) {
    const [open, setOpen] = useState(false);
    return (
      <>
        <button onClick={() => setOpen(true)}>Auf</button>
        {open ? (
          <Modal title="Titel" onClose={() => { setOpen(false); onClose(); }} footer={<button>Eins</button>}>
            <input aria-label="Feld" data-autofocus />
          </Modal>
        ) : null}
      </>
    );
  }
  test("startet auf data-autofocus, fängt Tab ein, Esc schließt und gibt den Fokus zurück", async () => {
    const { rt } = await runtime();
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(withRuntime(rt, <Host onClose={onClose} />));
    const opener = screen.getByRole("button", { name: "Auf" });
    await user.click(opener);
    const dialog = screen.getByRole("dialog", { name: "Titel" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByLabelText("Feld")).toHaveFocus();
    for (let i = 0; i < 5; i += 1) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(opener).toHaveFocus();
  });
});

describe("Modal Esc am Dokument", () => {
  test("schließt den Dialog auch bei Fokus außerhalb, und nur den obersten", async () => {
    const { rt } = await runtime();
    const user = userEvent.setup();
    const closeA = vi.fn();
    const closeB = vi.fn();
    function Stack({ both }: { both: boolean }) {
      return (
        <>
          <button>Draußen</button>
          <Modal title="A" onClose={closeA}><button>a</button></Modal>
          {both ? <Modal title="B" onClose={closeB}><button>b</button></Modal> : null}
        </>
      );
    }
    const view = render(withRuntime(rt, <Stack both />));
    act(() => screen.getByRole("button", { name: "Draußen" }).focus());
    await user.keyboard("{Escape}");
    expect(closeB).toHaveBeenCalledTimes(1);
    expect(closeA).not.toHaveBeenCalled();
    view.rerender(withRuntime(rt, <Stack both={false} />));
    act(() => document.body.focus());
    await user.keyboard("{Escape}");
    expect(closeA).toHaveBeenCalledTimes(1);
    expect(closeB).toHaveBeenCalledTimes(1);
    view.unmount();
    await user.keyboard("{Escape}");
    expect(closeA).toHaveBeenCalledTimes(1);
  });

  test("Esc im Dialog schließt genau einmal", async () => {
    const { rt } = await runtime();
    const user = userEvent.setup();
    const close = vi.fn();
    render(withRuntime(rt, <Modal title="A" onClose={close}><button>a</button></Modal>));
    await user.keyboard("{Escape}");
    expect(close).toHaveBeenCalledTimes(1);
  });
});

describe("Toasts", () => {
  test("Meldungen erscheinen, Fehler sind role=alert, Klick und Zeit entfernen sie", async () => {
    const { rt } = await runtime();
    render(withRuntime(rt, <Toasts />));
    vi.useFakeTimers();
    try {
      act(() => {
        rt.controller.toast("Hallo");
        rt.controller.toast("Kaputt", "error");
      });
      expect(screen.getByRole("status")).toHaveTextContent("Hallo");
      expect(screen.getByRole("alert")).toHaveTextContent("Kaputt");
      fireEvent.click(screen.getByText("Hallo"));
      expect(screen.queryByText("Hallo")).toBeNull();
      act(() => rt.controller.toast("Info"));
      act(() => vi.advanceTimersByTime(50));
      expect(screen.queryByText("Info")).toBeNull();
      act(() => vi.advanceTimersByTime(5949));
      expect(screen.getByRole("alert")).toHaveTextContent("Kaputt");
      act(() => vi.advanceTimersByTime(1));
      expect(screen.queryByText("Kaputt")).toBeNull();
    } finally {
      rt.controller.dispose();
      vi.useRealTimers();
    }
  });
});

const status = (patch: Partial<JobStatus>): JobStatus => ({
  id: 1, name: "Kopieren: a.txt", state: "running", filesDone: 0, bytesDone: 500, bytesTotal: 1000,
  bytesPerSecond: 2048, etaSeconds: null, queuePosition: null, lane: null, failed: [], skipped: [], warnings: [], ...patch,
});

describe("JobsPanel", () => {
  test("zeigt Fortschritt, Abbrechen ruft die API; beendet mit Fehlern: Zusammenfassung und Gründe", async () => {
    const { api, rt } = await runtime();
    const cancel = vi.spyOn(api, "cancelJob").mockResolvedValue();
    const user = userEvent.setup();
    render(withRuntime(rt, <JobsPanel />));
    expect(screen.queryByRole("region", { name: "Aufträge" })).toBeNull();
    act(() => rt.store.dispatch({ type: "job", event: { type: "job", status: status({}) } }));
    const panel = within(screen.getByRole("region", { name: "Aufträge" }));
    expect(panel.getByRole("progressbar")).toHaveAttribute("value", "0.5");
    expect(panel.getByText("500 B von 1000 B")).toBeInTheDocument();
    await user.click(panel.getByRole("button", { name: "Auftrag abbrechen" }));
    expect(cancel).toHaveBeenCalledWith(1);
    act(() => {
      rt.store.dispatch({ type: "jobKind", id: 1, kind: "copy" });
      rt.store.dispatch({
        type: "job",
        event: { type: "job", status: status({ state: "failed", filesDone: 117, bytesDone: 1000, failed: [
          { path: "/a/x", message: "Zugriff verweigert" }, { path: "/a/y", message: "Datei zu groß" }, { path: "/a/z", message: "E/A-Fehler" },
        ] }) },
      });
    });
    expect(panel.getByText("3 von 120 nicht kopiert")).toBeInTheDocument();
    expect(panel.getByText(/Grund: Datei zu groß/)).toBeInTheDocument();
    await user.click(panel.getByRole("button", { name: "Entfernen" }));
    expect(screen.queryByRole("region", { name: "Aufträge" })).toBeNull();
  });

  test("englische Oberfläche: Auftragsname und Fehlergründe kommen aus i18n, nicht aus dem Kerntext", async () => {
    const { rt } = await runtime("en");
    render(withRuntime(rt, <JobsPanel />));
    act(() => {
      rt.store.dispatch({ type: "jobKind", id: 1, kind: "copy" });
      rt.store.dispatch({
        type: "job",
        event: { type: "job", status: status({ name: "Kopieren: a.txt (+2)", title: { kind: "copy", first: "a.txt", more: 2 }, state: "failed", filesDone: 2, failed: [
          { path: "/a/x", message: "Zugriff verweigert: /a/x", kind: "permission" }, { path: "/a/y", message: "Roher Text", kind: "io" },
        ] }) },
      });
    });
    const panel = within(screen.getByRole("region", { name: "Jobs" }));
    expect(panel.getByRole("group", { name: "Copy: a.txt (+2)" })).toBeInTheDocument();
    expect(panel.getByText(/Reason: Access denied\./)).toBeInTheDocument();
    expect(panel.getByText(/Reason: Roher Text/)).toBeInTheDocument();
    expect(screen.queryByText(/Zugriff verweigert/)).toBeNull();
    expect(screen.queryByText(/Kopieren/)).toBeNull();
  });

  test("Papierkorb-Aufträge erscheinen nicht", async () => {
    const { rt } = await runtime();
    render(withRuntime(rt, <JobsPanel />));
    act(() => {
      rt.store.dispatch({ type: "jobKind", id: 1, kind: "trash" });
      rt.store.dispatch({ type: "job", event: { type: "job", status: status({ state: "done" }) } });
    });
    expect(screen.queryByRole("region", { name: "Aufträge" })).toBeNull();
  });
});

describe("DeleteDialog und ConflictDialog (Standardknöpfe)", () => {
  test("Löschdialog: Standardfokus „Abbrechen“, Zahlen und „… und n weitere“", async () => {
    const { rt } = await runtime();
    const model = deleteDialogModel({ token: "t1", files: 8, dirs: 2, links: 1, bytes: 2048, firstNames: ["a", "b", "c"], incomplete: true });
    render(withRuntime(rt, <DeleteDialog model={model} reason="permanent" />));
    const dlg = screen.getByRole("alertdialog", { name: "Endgültig löschen?" });
    expect(within(dlg).getByRole("button", { name: "Abbrechen" })).toHaveFocus();
    expect(within(dlg).getByText("8 Dateien, 2 Ordner, 1 Link")).toBeInTheDocument();
    expect(within(dlg).getByText("… und 8 weitere")).toBeInTheDocument();
    expect(within(dlg).getByText(/nicht vollständig ermittelt/)).toBeInTheDocument();
    expect(within(dlg).getByText("Gesamtgröße: 2,0 KB")).toBeInTheDocument();
  });

  test("Konfliktdialog: Standardfokus „Überspringen“, Antwort geht mit „Für alle“ an die API", async () => {
    const { api, rt } = await runtime();
    const answer = vi.spyOn(api, "answerConflict").mockResolvedValue();
    const user = userEvent.setup();
    const file = (size: number) => ({ name: "x.txt", kind: "file" as const, size, modified: 0, mode: 0o644, hidden: false, linkTarget: null, brokenLink: false, error: null });
    const conflict: ConflictInfo = { jobId: 7, sourcePath: "/a/x.txt", destinationPath: "/b/x.txt", source: file(10), destination: file(20) };
    render(withRuntime(rt, <ConflictDialog conflict={conflict} />));
    const dlg = screen.getByRole("alertdialog");
    expect(within(dlg).getByRole("button", { name: "Überspringen" })).toHaveFocus();
    await user.click(within(dlg).getByLabelText("Für alle"));
    await user.click(within(dlg).getByRole("button", { name: "Beide behalten" }));
    expect(answer).toHaveBeenCalledWith(7, { resolution: "keepBoth", applyToAll: true });
    expect(rt.store.getState().conflicts).toHaveLength(0);
  });

  test("Esc mit Fokus außerhalb schließt den Dialog trotz globalem Tastaturhandler", async () => {
    const { rt } = await runtime();
    const user = userEvent.setup();
    const close = vi.fn();
    function Host({ open }: { open: boolean }) {
      useGlobalKeys(rt.store, rt.controller);
      return (
        <>
          <button>Draußen</button>
          {open ? <Modal title="A" onClose={close}><button>a</button></Modal> : null}
        </>
      );
    }
    // Wie in der App: Der globale Handler ist zuerst registriert, der Dialog öffnet später.
    const view = render(withRuntime(rt, <Host open={false} />));
    view.rerender(withRuntime(rt, <Host open />));
    act(() => screen.getByRole("button", { name: "Draußen" }).focus());
    await user.keyboard("{Escape}");
    expect(close).toHaveBeenCalledTimes(1);
  });
});
