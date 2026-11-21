import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { HOME, setupApp } from "./test-utils";
import { CoreError } from "./ipc/api";
const ROOT = `${HOME}/Vergleich/links`;
async function setup(wrap?: NonNullable<Parameters<typeof setupApp>[0]>["wrap"]) {
  return setupApp({
    left: ROOT,
    compareDemo: true,
    wrap
  });
}
const usage = () => screen.getByRole("dialog", {
  name: "Speicherbelegung"
});
const dupes = () => screen.getByRole("dialog", {
  name: "Duplikate finden …"
});
test("usage start, progress, select, keyboard drilldown/back, trash confirmation and refresh", async () => {
  const trash = vi.fn();
  const app = await setup(api => ({
    ...api,
    trash: paths => {
      trash(paths);
      return api.trash(paths);
    }
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Speicherbelegung …" }));
  expect(within(usage()).getByRole("checkbox")).toBeChecked();
  expect(within(usage()).getByRole("button", {
    name: "In den Papierkorb"
  })).toBeDisabled();
  await app.user.click(within(usage()).getByRole("button", {
    name: "Messen"
  }));
  expect(within(usage()).getByRole("progressbar")).toBeInTheDocument();
  await app.runJobs();
  const list = await within(usage()).findByRole("listbox");
  const rows = within(list).getAllByRole("option");
  expect(rows.length).toBeGreaterThan(0);
  fireEvent.keyDown(list, {
    key: "ArrowDown"
  });
  expect(rows[0]).toHaveAttribute("aria-selected", "true");
  const folder = rows.find(r => r.textContent?.includes("ordner-nur"))!;
  await app.user.click(folder);
  fireEvent.keyDown(list, {
    key: "Enter"
  });
  await waitFor(() => expect(within(usage()).getByRole("button", {
    name: "Zurück"
  })).toBeEnabled());
  fireEvent.keyDown(within(usage()).getByRole("listbox"), {
    key: "Backspace"
  });
  await waitFor(() => expect(within(usage()).getByRole("button", {
    name: "Zurück"
  })).toBeDisabled());
  const row = within(usage()).getAllByRole("option").find(r => r.textContent?.includes("nur-links.txt"))!;
  await app.user.click(row);
  await app.user.click(within(usage()).getByRole("button", {
    name: "In den Papierkorb"
  }));
  expect(trash).not.toHaveBeenCalled();
  const confirm = screen.getByRole("alertdialog");
  expect(confirm).toHaveTextContent("Lässt sich im Verlauf rückgängig machen");
  expect(confirm).toContainElement((document.activeElement as HTMLElement));
  await app.user.click(within(confirm).getByRole("button", {
    name: "Ja, Papierkorb"
  }));
  await waitFor(() => expect(trash).toHaveBeenCalledWith([`${ROOT}/nur-links.txt`]));
  await app.runJobs();
});
test("usage escape closes and frees active session, errors are visible", async () => {
  const close = vi.fn();
  const app = await setup(api => ({
    ...api,
    usageClose: id => {
      close(id);
      return api.usageClose(id);
    }
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Speicherbelegung …" }));
  await app.user.click(within(usage()).getByRole("button", {
    name: "Messen"
  }));
  await app.user.keyboard("{Escape}");
  await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole("dialog")).toBeNull();
  await app.runJobs();
});
test("scan input error remains in dialog", async () => {
  const app = await setup(api => ({
    ...api,
    usageStart: async () => {
      throw new CoreError("Nur lokale Ordner", "invalidInput");
    }
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Speicherbelegung …" }));
  await app.user.click(within(usage()).getByRole("button", {
    name: "Messen"
  }));
  expect(await within(usage()).findByRole("alert")).toHaveTextContent("Nur lokale Ordner");
});
test("dupes roots, progress, keep rule, radio choice, confirmation sends only group and keeper", async () => {
  const trash = vi.fn(),
    suggest = vi.fn();
  const app = await setup(api => ({
    ...api,
    dupesSuggest: (...args) => {
      suggest(...args);
      return api.dupesSuggest(...args);
    },
    dupesTrash: async (id, choices) => {
      trash(id, choices);
      return api.dupesTrash(id, choices);
    }
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Duplikate finden …" }));
  expect(dupes()).toHaveTextContent("Nichts verlässt den Rechner.");
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Ordner hinzufügen"
  }));
  const root = within(dupes()).getByRole("textbox", {
    name: "Ordner 2"
  });
  await app.user.clear(root);
  await app.user.type(root, `${HOME}/Vergleich/rechts`);
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Suche starten"
  }));
  expect(within(dupes()).getByRole("progressbar")).toBeInTheDocument();
  await app.runJobs();
  const rules = await within(dupes()).findByRole("combobox");
  await app.user.selectOptions(rules, "newest");
  await waitFor(() => expect(suggest).toHaveBeenCalledWith(expect.any(Number), "newest", ROOT));
  const radios = within(dupes()).getAllByRole("radio");
  await app.user.click(radios[1]!);
  expect(radios[1]).toBeChecked();
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Überzählige in den Papierkorb"
  }));
  expect(trash).not.toHaveBeenCalled();
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", {
    name: "Ja, Papierkorb"
  }));
  await waitFor(() => expect(trash).toHaveBeenCalledTimes(1));
  for (const choice of trash.mock.calls[0]![1]) expect(Object.keys(choice).sort()).toEqual(["groupIndex", "keepPath"]);
  await app.runJobs();
});
test("dupes cancellation and focus trap", async () => {
  const app = await setup();
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Duplikate finden …" }));
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Suche starten"
  }));
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Abbrechen"
  }));
  await app.runJobs();
  await waitFor(() => expect(within(dupes()).queryByRole("progressbar")).toBeNull());
  const close = within(dupes()).getByRole("button", {
    name: "Schließen"
  });
  close.focus();
  await app.user.keyboard("{Shift>}{Tab}{/Shift}");
  expect(dupes()).toContainElement((document.activeElement as HTMLElement));
  await app.user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
});
test("dupes paginates while global confirmation includes unloaded groups", async () => {
  const sent = vi.fn();
  const group = (i: number) => ({
    groupIndex: i,
    size: 10,
    reclaimableBytes: 10,
    entries: [{
      path: `${ROOT}/${i}a`,
      modified: 1,
      protected: false
    }, {
      path: `${ROOT}/${i}b`,
      modified: 2,
      protected: false
    }]
  });
  const app = await setup(api => ({
    ...api,
    dupesPage: async (_id, offset, limit) => ({
      ready: true,
      progress: {
        stage: "fullHash",
        entries: 102,
        bytes: 1020,
        total: 102
      },
      total: 51,
      reclaimableBytes: 510,
      groups: Array.from({
        length: 51
      }, (_, i) => group(i)).slice(offset, offset + limit),
      incomplete: false,
      issues: []
    }),
    dupesSuggest: async () => Array.from({
      length: 51
    }, (_, i) => ({
      groupIndex: i,
      keepPath: `${ROOT}/${i}a`,
      redundantCount: 1,
      redundantBytes: 10
    })),
    dupesTrash: async (id, choices) => {
      sent(id, choices);
      throw new CoreError("Papierkorb-Testfehler", "io");
    }
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Duplikate finden …" }));
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Suche starten"
  }));
  await app.runJobs();
  await waitFor(() => expect(within(dupes()).getAllByRole("radio")).toHaveLength(100));
  await waitFor(() => expect(within(dupes()).getByRole("button", { name: "Überzählige in den Papierkorb" })).toBeEnabled());
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Überzählige in den Papierkorb"
  }));
  expect(screen.getByRole("alertdialog")).toHaveTextContent("51 Dateien");
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", {
    name: "Abbrechen"
  }));
  expect(sent).not.toHaveBeenCalled();
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Weitere Gruppen laden"
  }));
  expect(within(dupes()).getAllByRole("radio")).toHaveLength(102);
  await app.user.click(within(dupes()).getByRole("button", {
    name: "Überzählige in den Papierkorb"
  }));
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", {
    name: "Ja, Papierkorb"
  }));
  await waitFor(() => expect(sent.mock.calls[0]![1]).toHaveLength(51));
  expect(await within(dupes()).findByRole("alert")).toHaveTextContent("Papierkorb-Testfehler");
});
test("usage handles cancellation", async () => {
  const app = await setup();
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Speicherbelegung …" }));
  await app.user.click(within(usage()).getByRole("button", {
    name: "Messen"
  }));
  await app.user.click(within(usage()).getByRole("button", {
    name: "Abbrechen"
  }));
  await app.runJobs();
  expect(await within(usage()).findByRole("alert")).toHaveTextContent("Die Messung wurde abgebrochen.");
});
test("usage other is inert, opening selected folder navigates active pane", async () => {
  const app = await setup(api => ({
    ...api,
    usageNode: async (id, path) => {
      const p = await api.usageNode(id, path);
      return p.ready ? {
        ...p,
        other: {
          size: 20,
          allocated: 24,
          files: 2,
          directories: 0
        }
      } : p;
    }
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Speicherbelegung …" }));
  await app.user.click(within(usage()).getByRole("button", {
    name: "Messen"
  }));
  await app.runJobs();
  const other = await within(usage()).findByRole("option", {
    name: /Sonstiges/
  });
  expect(other).toHaveAttribute("aria-disabled", "true");
  await app.user.click(other);
  expect(within(usage()).getByRole("button", {
    name: "Im Fenster öffnen"
  })).toBeDisabled();
  const folder = within(usage()).getAllByRole("option").find(r => r.textContent?.includes("ordner-nur"))!;
  await app.user.click(folder);
  await app.user.click(within(usage()).getByRole("button", {
    name: "Im Fenster öffnen"
  }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const state = app.runtime.store.getState();
  expect(state.windows[0].tabs[state.windows[0].activeTab]!.path).toContain("ordner-nur");
});

test("usage refreshes after partial trash failure and keeps the failure visible", async () => {
  const start = vi.fn();
  let failedId: number | null = null;
  const app = await setup(api => ({
    ...api,
    usageStart: (path, options) => { start(path); return api.usageStart(path, options); },
    trash: async paths => { failedId = await api.trash(paths); return failedId; },
    jobs: async () => (await api.jobs()).map(job => job.id === failedId ? {
      ...job, state: "failed" as const, failed: [{ path: ROOT, message: "Test failure" }]
    } : job),
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Speicherbelegung …" }));
  await app.user.click(within(usage()).getByRole("button", { name: "Messen" }));
  await app.runJobs();
  const row = (await within(usage()).findAllByRole("option")).find(r => r.textContent?.includes("nur-links.txt"))!;
  await app.user.click(row);
  await app.user.click(within(usage()).getByRole("button", { name: "In den Papierkorb" }));
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Ja, Papierkorb" }));
  await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
  expect(await within(usage()).findByRole("alert")).toHaveTextContent("Test failure");
  await app.runJobs();
});

test.each(["cancelled", "failed"] as const)("dupes %s partial results cannot be trashed", async state => {
  let ready = false;
  const app = await setup(api => ({ ...api,
    dupesPage: async (...args) => { const p = await api.dupesPage(...args); ready = p.ready; return p; },
    jobs: async () => (await api.jobs()).map(j => ({ ...j, state: ready ? state : j.state })),
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Duplikate finden …" }));
  await app.user.click(within(dupes()).getByRole("button", { name: "Suche starten" }));
  await app.runJobs();
  await waitFor(() => expect(within(dupes()).getByRole("alert")).toBeVisible());
  expect(within(dupes()).getByRole("button", { name: "Überzählige in den Papierkorb" })).toBeDisabled();
  if (state === "cancelled") expect(dupes()).toHaveTextContent("Die Suche wurde abgebrochen.");
});

test("dupes incomplete results remain usable after cancellation and a fresh search", async () => {
  let cancelled = true;
  const app = await setup(api => ({ ...api,
    dupesPage: async (...args) => ({ ...(await api.dupesPage(...args)), cancelled, incomplete: true }),
  }));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Duplikate finden …" }));
  await app.user.click(within(dupes()).getByRole("button", { name: "Suche starten" }));
  await app.runJobs();
  await waitFor(() => expect(dupes()).toHaveTextContent("Die Suche wurde abgebrochen."));
  expect(within(dupes()).getByRole("button", { name: "Überzählige in den Papierkorb" })).toBeDisabled();
  cancelled = false;
  await app.user.click(within(dupes()).getByRole("button", { name: "Suche starten" }));
  await app.runJobs();
  await waitFor(() => expect(within(dupes()).getByRole("button", { name: "Überzählige in den Papierkorb" })).toBeEnabled());
  expect(dupes()).toHaveTextContent("unvollständig");
  expect(dupes()).not.toHaveTextContent("Die Suche wurde abgebrochen.");
});
