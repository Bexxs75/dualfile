import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, test } from "vitest";
import css from "./styles/app.css?raw";
import { DOCS, HOME, setupApp } from "./test-utils";
import { activeTabOf, visibleEntries } from "./app/state";
import { formatBytes } from "./i18n";

test("parent entry defaults on, toggles in settings and persists without moving selection", async () => {
  const app = await setupApp({ left: DOCS });
  expect(app.names(0)).toContain("..");
  const name = app.names(0).find(n => n !== "..")!;
  await app.user.click(app.row(0, name));
  await app.user.click(screen.getByRole("button", { name: /Einstellungen/ }));
  const toggle = screen.getByRole("checkbox", { name: "Eintrag „..“ anzeigen" });
  expect(toggle).toBeChecked();
  await app.user.click(toggle);
  expect(app.names(0)).not.toContain("..");
  expect(app.focused(0)).toBe(name);
  await waitFor(async () => expect((await app.api.loadSettings()).settings.showParentEntry).toBe(false));
  await app.user.click(toggle);
  expect(app.names(0)).toContain("..");
  expect(app.focused(0)).toBe(name);
});

test.each(["button", "Backspace", "AltUp"])("up navigation remains with parent hidden: %s", async method => {
  const app = await setupApp({ left: DOCS, settings: { showParentEntry: false } });
  expect(app.names(0)).not.toContain("..");
  if (method === "button") await app.user.click(within(app.pane(0)).getByRole("button", { name: "Eine Ebene höher" }));
  else fireEvent.keyDown(within(app.pane(0)).getByRole("grid"), { key: method === "AltUp" ? "ArrowUp" : "Backspace", altKey: method === "AltUp" });
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(HOME));
});

test.each(["de", "en"] as const)("numeric columns, parent metadata and selection status (%s)", async language => {
  const app = await setupApp({ language, left: DOCS });
  const up = app.row(0, "..");
  expect(up.querySelector(".num")).toHaveTextContent(/^$/);
  expect(up.querySelector(".dt")).toHaveTextContent(/^$/);
  for (const row of app.rows(0)) {
    expect(row.querySelector(".num")).toHaveClass("tabular-nums");
    expect(row.querySelector(".dt")).toHaveClass("tabular-nums");
  }
  const entries = visibleEntries(app.runtime.store.getState(), 0).filter(e => e.name !== "..");
  const files = entries.filter(e => e.kind === "file").slice(0, 2);
  expect(files).toHaveLength(2);
  for (const file of files) fireEvent.click(app.row(0, file.name), { ctrlKey: true });
  const status = within(app.pane(0)).getByRole("status");
  expect(status).toHaveTextContent(language === "de" ? `2 von ${entries.length} markiert` : `2 of ${entries.length} selected`);
  expect(status).toHaveTextContent(formatBytes(files.reduce((n, f) => n + f.size, 0), language));
  expect(status.firstElementChild).toHaveClass("tabular-nums");
  expect(status).toHaveTextContent(language === "de" ? "Versteckte Dateien ausgeblendet" : "Hidden files not shown");
  await act(async () => app.runtime.controller.command("toggleHidden"));
  expect(status).toHaveTextContent(language === "de" ? "Versteckte Dateien sichtbar" : "Hidden files shown");
});

test("F8 badges keep trash/permanent commands and warning CSS is restricted to interaction", async () => {
  await setupApp();
  const bar = screen.getByRole("toolbar", { name: "Funktionstasten" });
  expect(within(bar).getByRole("button", { name: "F8 Papierkorb" }).querySelector("kbd")).toHaveTextContent("F8");
  expect(bar.querySelectorAll("kbd")).toHaveLength(8);
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]+)\}/g)].filter(([, selector, body]) => selector!.includes(".fkeys") && body!.includes("var(--warn)"));
  expect(rules).toHaveLength(2);
  for (const [, selectors] of rules) for (const selector of selectors!.split(",")) expect(selector).toMatch(/:hover|:focus-visible/);
  expect(css).toContain(".fkeys button:focus-visible { outline: 2px solid var(--accent)");
});

test("empty selection has an explicit zero sum", async () => {
  const app = await setupApp({ left: DOCS });
  expect(within(app.pane(0)).getByRole("status")).toHaveTextContent("Markiert: 0 B");
  expect(within(app.pane(1)).getByRole("status")).toHaveTextContent("Markiert: 0 B");
});
