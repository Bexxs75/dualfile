import { act, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { DOCS, DOWNLOADS, HOME, setupApp } from "./test-utils";

const headers = (pane: HTMLElement) => within(pane).getAllByRole("columnheader").map(h => h.textContent);
const cells = (row: HTMLElement) => within(row).getAllByRole("gridcell");

test("extension column: split names, raw extension cell, header order", async () => {
  const app = await setupApp({ left: DOCS, right: DOWNLOADS, settings: { extensionColumn: true } });
  expect(headers(app.pane(0))).toEqual(["Name", "Endung", "Größe", "Geändert"]);
  expect(app.pane(0).querySelector(".head")).toHaveClass("ext-split");
  const pdf = app.row(0, "rechnung-oktober");
  expect(pdf.className).toContain("ext-split");
  expect(pdf.querySelector(".n")?.getAttribute("class")).toBe("n");
  expect(cells(pdf)[1]).toHaveTextContent(/^pdf$/);
  expect(cells(pdf)[1]).toHaveClass("ex");
  expect(cells(pdf)[2]).toHaveClass("num");
  expect(cells(pdf)[3]).toHaveClass("dt");
  expect(pdf.querySelector(".nm")).toHaveAttribute("title", "rechnung-oktober.pdf");
  const archive = app.row(1, "rezept-paket.tar");
  expect(cells(archive)[1]).toHaveTextContent(/^gz$/);
});

test("extension column: dotfiles and directories stay whole with empty extension", async () => {
  const app = await setupApp({ left: HOME, settings: { extensionColumn: true } });
  act(() => app.runtime.store.dispatch({ type: "toggleHidden" }));
  const dot = app.row(0, ".bashrc");
  expect(cells(dot)[1]).toHaveTextContent(/^$/);
  const dir = app.row(0, "Dokumente");
  expect(cells(dir)[1]).toHaveTextContent(/^$/);
  expect(cells(app.row(0, "..")).length).toBe(4);
});

test("extension column off by default: Typ header and full names", async () => {
  const app = await setupApp({ left: DOCS });
  expect(headers(app.pane(0))).toEqual(["Name", "Größe", "Typ", "Geändert"]);
  expect(app.pane(0).querySelector(".ext-split")).toBeNull();
  const pdf = app.row(0, "rechnung-oktober.pdf");
  expect(cells(pdf)[2]).toHaveTextContent("PDF-Datei");
});

test("extension column setting toggles in the settings dialog", async () => {
  const app = await setupApp({ left: DOCS });
  await app.user.click(screen.getByRole("button", { name: /Einstellungen/ }));
  await app.user.click(screen.getByRole("tab", { name: "Dateiliste" }));
  const toggle = screen.getByRole("switch", { name: "Dateiendung in eigener Spalte" });
  expect(toggle).not.toBeChecked();
  await app.user.click(toggle);
  expect(headers(app.pane(0))[1]).toBe("Endung");
});
