/// <reference types="vite/client" />
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { DOCS, DOWNLOADS, HOME, setupApp } from "./test-utils";
import { activeTabOf } from "./app/state";
import { isModalOpen } from "./components/Modal";
import { CoreError } from "./ipc/api";
import css from "./styles/app.css?raw";
import { contrastRatio, themes, themeNames } from "./themes/tokens";

afterEach(() => vi.useRealTimers());

test("B19 Escape releases modal, immediate path Return navigates and list Tab switches panes", async () => {
  const app = await setupApp();
  for (const kind of ["settings", "makeDir", "pattern"] as const) {
    await app.user.keyboard(kind === "settings" ? "{Control>},{/Control}" : kind === "makeDir" ? "{F7}" : "+");
    expect(isModalOpen()).toBe(true);
    await app.user.keyboard("{Escape}");
    expect(isModalOpen()).toBe(false);
    fireEvent.click(within(app.pane(0)).getByRole("navigation"));
    const input = within(app.pane(0)).getByRole("textbox", { name: "Pfad" });
    fireEvent.change(input, { target: { value: DOCS } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
    act(() => within(app.pane(0)).getByRole("grid").focus());
    await app.user.keyboard("{Tab}");
    expect(app.runtime.store.getState().active).toBe(1);
    await app.user.keyboard("{Tab}");
    await act(async () => { await app.runtime.controller.navigateTo(0, HOME); });
  }
});

test("B21 dblclick opens folder; denied navigation reports error for at least six seconds", async () => {
  const app = await setupApp({ options: { toastMs: 500 }, wrap: api => ({ ...api, list: async path => {
    if (path === DOCS) throw new CoreError("denied", "permission");
    return api.list(path);
  } }) });
  vi.useFakeTimers();
  await act(async () => { fireEvent.doubleClick(app.row(0, "Dokumente")); });
  expect(app.runtime.store.getState().toasts).toHaveLength(1);
  expect(app.runtime.store.getState().toasts[0]?.kind).toBe("error");
  expect(app.runtime.store.getState().toasts[0]?.text).toBe(`Zugriff verweigert: ${DOCS}`);
  act(() => vi.advanceTimersByTime(5999));
  expect(app.runtime.store.getState().toasts).toHaveLength(1);
  act(() => vi.advanceTimersByTime(1));
  expect(app.runtime.store.getState().toasts).toHaveLength(0);
});

test("B21 dblclick on readable directory navigates", async () => {
  const app = await setupApp();
  fireEvent.doubleClick(app.row(0, "Dokumente"));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
});

test("B16 middle ellipsis retains suffix and tooltip, inactive cursor is weaker", async () => {
  const app = await setupApp();
  const name = "systemd-private-123456789012345678901234567890-chrony.service-x7Kq";
  await act(async () => { await app.api.makeDir(HOME, name); await app.runtime.controller.reload(0); });
  const label = app.row(0, name).querySelector(".nm")!;
  expect(label).toHaveAttribute("title", name);
  expect(label.querySelector(".nm-end")).toHaveTextContent("service-x7Kq");
  expect(css).toMatch(/\.pane:not\(\.active\) \.row.cur\s*\{[^}]*border-left-color: var\(--muted\)/);
});

test("B17 row menu retains multi-selection; blank menu uses folder actions only", async () => {
  const app = await setupApp({ left: DOWNLOADS });
  fireEvent.click(app.row(0, "Datei1.txt"));
  fireEvent.click(app.row(0, "Datei2.txt"), { ctrlKey: true });
  fireEvent.contextMenu(app.row(0, "Datei1.txt"));
  expect(app.selected(0)).toEqual(["Datei1.txt", "Datei2.txt"]);
  expect(screen.getByRole("menuitem", { name: "Öffnen" })).toBeDisabled();
  expect(screen.getByRole("menuitem", { name: "Umbenennen" })).toBeDisabled();
  expect(screen.queryByRole("menuitem", { name: "Verzeichnisse vergleichen …" })).toBeNull();
  await app.user.click(screen.getByRole("menuitem", { name: "Packen …" }));
  expect(app.dialog()).toHaveTextContent("Datei1.txt");
  expect(app.dialog()).toHaveTextContent("Datei2.txt");
  await app.user.keyboard("{Escape}");
  fireEvent.contextMenu(app.row(0, "Datei10.txt"));
  expect(app.selected(0)).toEqual(["Datei10.txt"]);
  await app.user.keyboard("{Escape}");
  fireEvent.contextMenu(within(app.pane(0)).getByRole("grid"));
  expect(screen.getByRole("menuitem", { name: "Verzeichnisse vergleichen …" })).toBeInTheDocument();
  expect(screen.getByRole("menuitem", { name: "Neuer Ordner" })).toBeInTheDocument();
  expect(screen.queryByRole("menuitem", { name: "Umbenennen" })).toBeNull();
});

test("B17 filter remembers hidden selections but actions never fall back to unrelated focus", async () => {
  const app = await setupApp({ left: DOWNLOADS });
  fireEvent.click(app.row(0, "Datei1.txt"));
  fireEvent.click(app.row(0, "Datei2.txt"), { ctrlKey: true });
  const filter = within(app.pane(0)).getByRole("searchbox");
  fireEvent.change(filter, { target: { value: "Datei1" } });
  expect([...activeTabOf(app.runtime.store.getState(), 0).selection.selected]).toEqual(["Datei1.txt", "Datei2.txt"]);
  act(() => app.runtime.controller.command("copy"));
  expect(app.dialog()).toHaveTextContent("Datei1.txt");
  expect(app.dialog()).not.toHaveTextContent("Datei2.txt");
  await app.user.keyboard("{Escape}");
  fireEvent.change(filter, { target: { value: "anleitung" } });
  act(() => app.runtime.controller.command("copy"));
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.change(filter, { target: { value: "" } });
  expect(app.selected(0)).toEqual(["Datei1.txt", "Datei2.txt"]);
});

test("B23 Jobs title opens without collapsing; explicit collapse and bounded scrolling", async () => {
  const app = await setupApp({ left: DOCS });
  fireEvent.click(app.row(0, "notizen.txt"));
  act(() => app.runtime.controller.command("copy"));
  await app.user.click(within(app.dialog()).getByRole("button", { name: "Kopieren" }));
  const title = screen.getByRole("button", { name: /^Aufträge ·/ });
  await app.user.click(title);
  expect(title).toHaveAttribute("aria-expanded", "true");
  await app.user.click(screen.getByRole("button", { name: "Aufträge einklappen" }));
  expect(title).toHaveAttribute("aria-expanded", "false");
  await app.user.click(title);
  expect(title).toHaveAttribute("aria-expanded", "true");
  expect(css).toMatch(/\.jobs\s*\{[^}]*max-height: 25vh/);
  expect(css).toMatch(/#jobs-list:not\(\[hidden\]\)\s*\{[^}]*overflow: auto/);
});

test("B28 rename reserves status space and sizes to preview; session-only preset wording", async () => {
  const app = await setupApp({ left: DOWNLOADS });
  fireEvent.click(app.row(0, "Datei1.txt"));
  fireEvent.click(app.row(0, "Datei2.txt"), { ctrlKey: true });
  await app.user.keyboard("{Control>}m{/Control}");
  expect(app.dialog()).toHaveTextContent("nicht dauerhaft gespeichert");
  expect(css).toMatch(/\.mr-summary\s*\{[^}]*min-height: 2.6em/);
  expect(css).toMatch(/\.dlg.mr\s*\{[^}]*height: auto/);
});

test("B29 quick-view panel header follows current source path", async () => {
  const app = await setupApp({ left: DOCS });
  fireEvent.click(app.row(0, "notizen.txt"));
  await app.user.keyboard("{Control>}q{/Control}");
  expect(app.pane(1).querySelector(".quick-path")).toHaveTextContent(`${DOCS}/notizen.txt`);
  expect(css).toMatch(/\.lv-viewer kbd\s*\{[^}]*font-size: 12px/);
});

test.each(themeNames)("B31 %s muted text on all normal surfaces meets AA and placeholder meets 3", name => {
  const palette = themes[name];
  for (const background of [palette.bg, palette.panel, palette.panel2]) {
    expect(contrastRatio(palette.muted, background)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(palette.fg, background)).toBeGreaterThanOrEqual(4.5);
  }
  expect(contrastRatio(palette.muted, palette.bg)).toBeGreaterThanOrEqual(3);
  expect(css).toMatch(/\.filter::placeholder\s*\{[^}]*color: var\(--muted\)[^}]*opacity: 1/);
});

test("B27 natural order spans pages; hidden setting and summary follow panels", async () => {
  const app = await setupApp({ left: HOME, right: DOWNLOADS });
  await act(async () => {
    await app.api.makeDir(HOME, "p4-left");
    await app.api.makeDir(HOME, "p4-right");
    for (let i = 1; i <= 105; i++) await app.api.makeDir(`${HOME}/p4-left`, `datei${i}`);
    await app.api.makeDir(`${HOME}/p4-left`, ".hidden");
    await app.api.makeDir(`${HOME}/p4-left/.hidden`, "child");
    await app.runtime.controller.navigateTo(0, `${HOME}/p4-left`);
    await app.runtime.controller.navigateTo(1, `${HOME}/p4-right`);
    app.runtime.controller.openCompare();
  });
  expect(screen.getByRole("checkbox", { name: "Versteckte einbeziehen" })).not.toBeChecked();
  await act(async () => { await app.runtime.controller.startCompare(); });
  await app.runJobs();
  await waitFor(() => expect(app.runtime.store.getState().compare?.phase).toBe("ready"));
  let page = app.runtime.store.getState().compare!.page!;
  expect(page.entries.slice(0, 3).map(e => e.rel)).toEqual(["datei1", "datei2", "datei3"]);
  expect(page.total).toBe(105);
  expect(page.summary.entries).toBe(105);
  await act(async () => { await app.runtime.controller.compareGoto(100); });
  page = app.runtime.store.getState().compare!.page!;
  expect(page.entries.map(e => e.rel)).toEqual(["datei101", "datei102", "datei103", "datei104", "datei105"]);
  await app.user.click(screen.getByRole("checkbox", { name: "Versteckte einbeziehen" }));
  await waitFor(() => expect(app.runtime.store.getState().compare!.page!.total).toBe(107));
  expect(app.runtime.store.getState().compare!.page!.entries[0]?.rel).toBe(".hidden");
  act(() => { app.runtime.controller.closeCompare(); app.runtime.controller.command("toggleHidden"); app.runtime.controller.openCompare(); });
  expect(screen.getByRole("checkbox", { name: "Versteckte einbeziehen" })).toBeChecked();
});

test("B27 sync excludes hidden paths and their directory actions even after select-all", async () => {
  const plans: (string[] | undefined)[] = [];
  const app = await setupApp({ wrap: api => ({ ...api, planSync: async (id, mode, options, checked) => {
    plans.push(checked);
    return api.planSync(id, mode, options, checked);
  } }) });
  await act(async () => {
    await app.api.makeDir(HOME, "p4-left");
    await app.api.makeDir(HOME, "p4-right");
    await app.api.makeDir(`${HOME}/p4-left`, ".hidden");
    await app.api.makeDir(`${HOME}/p4-left`, "visible");
    await app.api.makeDir(`${HOME}/p4-right`, "protected");
    await app.api.makeDir(`${HOME}/p4-right/protected`, ".keep");
    await app.runtime.controller.navigateTo(0, `${HOME}/p4-left`);
    await app.runtime.controller.navigateTo(1, `${HOME}/p4-right`);
    app.runtime.controller.openCompare();
    await app.runtime.controller.startCompare();
  });
  await app.runJobs();
  await waitFor(() => expect(app.runtime.store.getState().compare?.phase).toBe("ready"));
  act(() => app.runtime.controller.openSync());
  await waitFor(() => expect(app.runtime.store.getState().compare?.sync?.phase).toBe("preview"));
  act(() => app.runtime.controller.setSyncMode("mirror"));
  await waitFor(() => expect(app.runtime.store.getState().compare?.sync?.phase).toBe("preview"));
  await waitFor(() => expect(plans.at(-1)).toEqual(["visible"]));
  act(() => app.runtime.controller.setSyncAll(true));
  await waitFor(() => expect(plans.at(-1)).toEqual(["visible"]));
  expect(app.runtime.store.getState().compare!.sync!.actions.map(a => a.rel)).toEqual(["visible"]);
});

test("B21 newer messages cannot evict an error before its timeout", async () => {
  const app = await setupApp({ options: { toastMs: 500 }, wrap: api => ({ ...api, list: async path => {
    if (path === DOCS) throw new CoreError("denied", "permission");
    return api.list(path);
  } }) });
  vi.useFakeTimers();
  await act(async () => {
    for (let i = 0; i < 5; i++) await app.runtime.controller.navigateTo(0, DOCS);
  });
  expect(app.runtime.store.getState().toasts).toHaveLength(5);
  act(() => vi.advanceTimersByTime(5999));
  expect(app.runtime.store.getState().toasts).toHaveLength(5);
  act(() => vi.advanceTimersByTime(1));
  expect(app.runtime.store.getState().toasts).toHaveLength(0);
});
