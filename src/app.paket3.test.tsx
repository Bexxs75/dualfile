/// <reference types="vite/client" />
import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { DOCS, DOWNLOADS, HOME, setupApp } from "./test-utils";
import css from "./styles/app.css?raw";
import { contrastRatio, themes } from "./themes/tokens";
import { archiveRootOf } from "./state/paths";

const settings = async (options: Parameters<typeof setupApp>[0] = {}) => {
  const app = await setupApp(options);
  await app.user.keyboard("{Control>},{/Control}");
  return app;
};

test("B07 filename minimum, ordered container breakpoints and full-name tooltip", async () => {
  const app = await setupApp({ left: DOCS });
  expect(app.row(0, "notizen.txt").querySelector(".nm")).toHaveAttribute("title", "notizen.txt");
  expect(css).toContain("minmax(180px, 1fr) 84px 104px 130px");
  expect(css).toMatch(/@container \(max-width: 620px\)[\s\S]*nth-child\(3\)[\s\S]*@container \(max-width: 500px\)[\s\S]*nth-child\(4\)/);
});

test("B08 automatic Paper contrast and explicit custom colors survive theme changes", async () => {
  const app = await settings();
  await app.user.click(screen.getByRole("button", { name: "Passend zum Design" }));
  await app.user.click(screen.getByRole("radio", { name: "Papier" }));
  const accent = document.documentElement.style.getPropertyValue("--accent");
  expect(contrastRatio(accent, themes.papier.bg)).toBeGreaterThanOrEqual(3);
  const input = screen.getByLabelText("Eigener Farbwert (#rrggbb)");
  await app.user.clear(input);
  await app.user.type(input, "#4f8cff");
  await app.user.click(screen.getByRole("radio", { name: "Schiefer" }));
  await app.user.click(screen.getByRole("radio", { name: "Papier" }));
  expect(document.documentElement.style.getPropertyValue("--accent")).toBe("#4f8cff");
  expect(css).toMatch(/\.warning-slot\s*\{[^}]*height: 1.5em/);
});

test("B09 compact header with tools and connections menus without content width cap", async () => {
  const app = await setupApp();
  expect(app.view.container.querySelectorAll(".header-toolbar > .header-menu")).toHaveLength(2);
  expect(css).toMatch(/\.app\s*\{[^}]*max-width: none/);
  expect(css).toMatch(/\.header-toolbar\s*\{[^}]*min-height: 44px/);
});

test.each([["de", "Speicherbelegung"], ["en", "Disk usage"]] as const)("B11 %s disk usage button matches title", async (language, title) => {
  const app = await setupApp({ language });
  await app.user.click(screen.getByRole("button", { name: language === "de" ? "Werkzeuge" : "Tools" }));
  await app.user.click(screen.getByRole("menuitem", { name: `${title} …` }));
  expect(screen.getByRole("dialog", { name: title })).toBeInTheDocument();
});

test("B13 bookmark tooltip follows pinned state", async () => {
  const app = await setupApp({ left: DOCS });
  const bar = within(app.pane(0));
  const pin = bar.getByRole("button", { name: /Zu Orten hinzufügen/ });
  expect(pin).toHaveAttribute("title", "Aktuellen Ordner zu Orten hinzufügen (Ctrl+D)");
  await app.user.click(pin);
  expect(bar.getByRole("button", { name: "Aus Orten entfernen" })).toHaveAttribute("title", "Aus Orten entfernen (Ctrl+D)");
});

test("B14 German design group and names", async () => {
  await settings();
  const group = screen.getByRole("radiogroup", { name: "Design" });
  expect(within(group).getAllByRole("radio").map(node => node.textContent)).toEqual(["Schiefer", "Papier", "Werkstatt", "Weiß"]);
});

test("B15 checksum help spans columns and warning slot always exists", async () => {
  const app = await settings();
  expect(app.dialog().querySelector(".warning-slot")).toBeInTheDocument();
  expect(css).toMatch(/\.settings \.field-help\s*\{[^}]*grid-column: 1 \/ -1/);
  const slot = app.dialog().querySelector(".warning-slot");
  await app.user.clear(screen.getByLabelText("Eigener Farbwert (#rrggbb)"));
  expect(app.dialog().querySelector(".warning-slot")).toBe(slot);
  await app.user.click(within(app.dialog()).getByRole("tab", { name: "Kopieren" }));
  expect(app.dialog().querySelector("#verify-help")).toHaveClass("field-help");
});

test("B17 selection size explicitly excludes folders, including folder-only selection", async () => {
  const app = await setupApp({ left: HOME });
  await app.user.click(app.row(0, "Dokumente"));
  expect(app.pane(0).querySelector(".foot")).toHaveTextContent("Ausgewählt: 0 B (ohne Ordner)");
  await act(async () => { await app.api.makeDir(DOCS, "folder"); await app.runtime.controller.navigateTo(0, DOCS); });
  await app.user.keyboard("{Control>}a{/Control}");
  expect(app.pane(0).querySelector(".foot")).toHaveTextContent("ohne Ordner");
});

test.each(["F5", "F6"])("B18 %s destination starts selected and Enter submits", async key => {
  const app = await setupApp({ left: DOCS });
  await app.user.click(app.row(0, "notizen.txt"));
  await app.user.keyboard(`{${key}}`);
  const input = within(app.dialog()).getByRole("textbox") as HTMLInputElement;
  expect(input).toHaveFocus();
  expect([input.selectionStart, input.selectionEnd]).toEqual([0, DOWNLOADS.length]);
  await app.user.keyboard("{Enter}");
  expect(screen.queryByRole("dialog")).toBeNull();
  await app.runJobs();
  expect(app.names(1)).toContain("notizen.txt");
});

test("B20 archive edit disabled for button and command while extraction remains", async () => {
  const spy = vi.fn();
  const app = await setupApp({ left: archiveRootOf(`${DOWNLOADS}/rezept-paket.tar.gz`), right: DOCS, wrap: api => ({ ...api, openExternal: spy }) });
  await app.user.click(app.row(0, "README.md"));
  expect(screen.getByRole("button", { name: /F4/ })).toBeDisabled();
  expect(screen.getByRole("button", { name: /F5/ })).toBeEnabled();
  await app.user.keyboard("{F4}");
  expect(spy).not.toHaveBeenCalled();
});

test("B22 undo toast/history name items and show platform shortcut without overlay", async () => {
  const platform = vi.spyOn(navigator, "platform", "get").mockReturnValue("Linux x86_64");
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.copy([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`], DOWNLOADS); });
  await app.runJobs();
  await waitFor(() => expect(app.view.container.querySelector(".undo-toast")).toHaveTextContent("Kopieren: notizen.txt und 1 weitere"));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Verlauf der Änderungen …" }));
  expect(app.dialog()).toHaveTextContent("Kopieren: notizen.txt und 1 weitere");
  expect(app.dialog()).toHaveTextContent("Ctrl+Z");
  expect(app.dialog()).not.toHaveTextContent("⌘");
  expect(css).toMatch(/\.toasts\s*\{[^}]*position: absolute;[^}]*pointer-events: none/);
  expect(css).toMatch(/\.toast\s*\{[^}]*background: #212932/);
  await app.user.keyboard("{Escape}");
  platform.mockReturnValue("MacIntel");
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Verlauf der Änderungen …" }));
  expect(app.dialog()).toHaveTextContent("Cmd+Z");
  expect(app.dialog()).not.toHaveTextContent("Ctrl+Z");
  platform.mockRestore();
});

test("B24 conflict includes full destination path and neutral overwrite", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.copy([`${DOCS}/notizen.txt`], DOWNLOADS); });
  await app.runJobs();
  await act(async () => { await app.api.copy([`${DOCS}/notizen.txt`], DOWNLOADS); });
  await app.runJobs();
  const dialog = app.alertDialog();
  expect(dialog).toHaveTextContent(`${DOWNLOADS}/notizen.txt`);
  expect(within(dialog).getByRole("button", { name: "Überschreiben" })).not.toHaveClass("primary", "warn");
  expect(within(dialog).getByRole("button", { name: "Überspringen" })).toHaveFocus();
});

test("B25 similar scan labels share spaced fields and review text", async () => {
  const app = await setupApp({ language: "en" });
  await app.user.click(screen.getByRole("button", { name: "Tools" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Similar images …" }));
  expect(screen.getByLabelText("Minimum size (bytes)").closest("label")).toHaveClass("fld");
  expect(screen.getByLabelText("Sensitivity").closest("label")).toHaveClass("fld");
  expect(app.dialog()).toHaveTextContent(": review every");
  expect(css).toContain('.space-dialog input[type="radio"] { accent-color: var(--accent); }');
});

test("B26 mirror hint reflects removals and replacement checkbox sits outside group", async () => {
  const app = await setupApp({ left: `${HOME}/Vergleich/links/gemeinsam`, right: `${HOME}/Vergleich/rechts/gemeinsam`, compareDemo: true });
  await app.user.keyboard("{Control>}{Shift>}D{/Shift}{/Control}");
  await app.user.click(within(app.dialog()).getByRole("button", { name: "Vergleichen" }));
  await app.runJobs();
  await app.user.click(within(app.dialog()).getByRole("button", { name: "Synchronisieren …" }));
  const dialog = app.alertDialog();
  await app.user.click(within(dialog).getByRole("radio", { name: /Spiegeln/ }));
  await waitFor(() => expect(dialog).toHaveTextContent("Zusätzliche Dateien und Links rechts werden in den Papierkorb gelegt"));
  expect(dialog).not.toHaveTextContent("Nichts wird überschrieben oder gelöscht");
  const checkbox = within(dialog).getByRole("checkbox", { name: /Neuere/ });
  expect(checkbox.closest("fieldset")).toBeNull();
});


test("B24 nested transfer conflict is relative to its confirmed destination", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.makeDir(DOCS, "nested"); await app.api.copy([`${DOCS}/notizen.txt`], `${DOCS}/nested`); });
  await app.runJobs();
  for (let attempt = 0; attempt < 2; attempt++) {
    await app.user.click(app.row(0, "nested"));
    await app.user.keyboard("{F5}{Enter}");
    await app.runJobs();
  }
  expect(app.alertDialog()).toHaveTextContent("nested/notizen.txt");
  expect(app.alertDialog()).not.toHaveTextContent(`${DOWNLOADS}/nested/notizen.txt`);
});
