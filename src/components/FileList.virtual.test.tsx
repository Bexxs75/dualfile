// Issue #63: the file list renders only a window of rows. Counts instead of timings, so the
// test is deterministic on any machine. jsdom has no layout: scrollTop is stubbed per element and
// scrollIntoView is replaced by what a browser would do with fixed-height rows.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime, type AppRuntime } from "../app/runtime";
import { activeTabOf, defaultSettings, visibleEntries } from "../app/state";
import { createMockApi } from "../ipc/mock";
import type { Entry, Settings } from "../ipc/types";
import { rowHeights } from "../themes/tokens";
import { ManualScheduler } from "../test-utils";
import { FileList, OVERSCAN_ROWS, rowId, rowWindow } from "./FileList";

const N = 50_000;
const LIMIT = 300;

const runtimes: AppRuntime[] = [];
const scrollTops = new WeakMap<Element, number>();
let scrolledTo: string[] = [];

beforeEach(() => {
  scrolledTo = [];
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get(this: HTMLElement) { return scrollTops.get(this) ?? 0; },
    set(this: HTMLElement, value: number) { scrollTops.set(this, value); },
  });
  // Browser stand-in: put the row at the top of the list (block "nearest" from far away). A real
  // browser fires the scroll event later (next frame), so tests fire it with `settle`.
  Element.prototype.scrollIntoView = function (this: Element) {
    scrolledTo.push(this.id);
    const grid = this.closest<HTMLElement>('[role="grid"]');
    const index = Number(/-row-(\d+)$/.exec(this.id)?.[1] ?? NaN);
    if (!grid || Number.isNaN(index)) return;
    grid.scrollTop = index * rowHeights.normal;
  };
});
afterEach(() => {
  runtimes.splice(0).forEach((r) => r.controller.dispose());
  delete (HTMLElement.prototype as { scrollTop?: number }).scrollTop;
  delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
});

function entry(name: string, i: number, kind: "file" | "dir" = "file"): Entry {
  return { name, kind, size: (i * 7919) % 100_000, modified: 1_500_000_000_000 + ((i * 104_729) % 300_000_000) * 1000, mode: 0o644, uid: 1000, gid: 1000, ownerName: "tester", groupName: "tester", permissions: "rw-r--r--", hidden: false, linkTarget: null, brokenLink: false, error: null };
}

/** 50,000 files f00000…f49999 (zero-padded, so name order equals index order after "..").  */
function bigListing(): Entry[] {
  const out: Entry[] = [];
  for (let i = 0; i < N; i++) out.push(entry(`f${String(i).padStart(5, "0")}.txt`, i));
  return out.reverse();
}

async function setup(patch: Partial<Settings> = {}) {
  const api = createMockApi({ scheduler: new ManualScheduler(), initialSettings: { ...defaultSettings, language: "en", ...patch } });
  const rt = createRuntime(api, { saveDelayMs: 1 });
  runtimes.push(rt);
  await rt.controller.start();
  render(<RuntimeProvider runtime={rt}><FileList w={0} /></RuntimeProvider>);
  const path = activeTabOf(rt.store.getState(), 0).path;
  act(() => rt.store.dispatch({ type: "listing", w: 0, path, entries: bigListing() }));
  return { rt, grid: screen.getByRole("grid") };
}

const rows = () => screen.getAllByRole("row").filter((r) => !r.classList.contains("head"));
const shown = (rt: AppRuntime) => visibleEntries(rt.store.getState(), 0);
const focus = (rt: AppRuntime) => activeTabOf(rt.store.getState(), 0).selection.focus;
/** Runs an action, then delivers the scroll event the browser would send afterwards. */
const settle = (grid: HTMLElement, action: () => void) => { act(action); act(() => { fireEvent.scroll(grid); }); };
const scrollTo = (grid: HTMLElement, row: number) => act(() => { grid.scrollTop = row * rowHeights.normal; fireEvent.scroll(grid); });

test("rowWindow covers viewport plus overscan and stays inside the list", () => {
  expect(rowWindow(N, 0, 840, 28)).toEqual({ first: 0, last: 31 + OVERSCAN_ROWS });
  expect(rowWindow(N, 1000, 840, 28)).toEqual({ first: 1000 - OVERSCAN_ROWS, last: 1031 + OVERSCAN_ROWS });
  // A stale position past the end (list got shorter) still renders the last rows.
  expect(rowWindow(10, 5000, 840, 28)).toEqual({ first: 0, last: 10 });
  expect(rowWindow(100, 5000, 840, 28)).toEqual({ first: 69 - OVERSCAN_ROWS, last: 100 });
  expect(rowWindow(0, 0, 840, 28)).toEqual({ first: 0, last: 0 });
});

test("50,000 entries render fewer than 300 rows; aria-rowcount counts all of them", async () => {
  const { rt, grid } = await setup();
  const count = shown(rt).length;
  expect(count).toBe(N + 1); // with ".."
  expect(rows().length).toBeGreaterThan(20);
  expect(rows().length).toBeLessThan(LIMIT);
  expect(grid).toHaveAttribute("aria-rowcount", String(count + 1));
  expect(rows()[0]).toHaveAttribute("aria-rowindex", "2");
  // Spacers keep the full scroll height: rendered rows plus spacer rows equal the entry count.
  const spacers = [...grid.querySelectorAll<HTMLElement>(".row-spacer")];
  const spacerRows = spacers.reduce((sum, s) => sum + parseFloat(s.style.height || "0"), 0) / rowHeights.normal;
  // jsdom's CSS parser keeps lengths in single precision (1398236px reads back as 1398240px).
  expect(Math.round(spacerRows) + rows().length).toBe(count);
});

test("scrolling, sorting and filtering keep the DOM under the limit", async () => {
  const { rt, grid } = await setup();
  scrollTo(grid, 25_000);
  expect(rows().length).toBeLessThan(LIMIT);
  const middle = document.getElementById(rowId(0, 25_010))!;
  expect(middle).toHaveAttribute("aria-rowindex", "25012");
  expect(middle.textContent).toContain(shown(rt)[25_010]!.name);

  act(() => rt.controller.sortBy(0, "size"));
  expect(rows().length).toBeLessThan(LIMIT);
  expect(document.getElementById(rowId(0, 25_010))!.textContent).toContain(shown(rt)[25_010]!.name);
  act(() => rt.controller.sortBy(0, "modified"));
  expect(rows().length).toBeLessThan(LIMIT);

  act(() => rt.controller.setFilter(0, "7"));
  const filtered = shown(rt).length;
  expect(filtered).toBeGreaterThan(1000);
  expect(filtered).toBeLessThan(N);
  expect(rows().length).toBeLessThan(LIMIT);
  expect(grid).toHaveAttribute("aria-rowcount", String(filtered + 1));

  act(() => rt.controller.setFilter(0, "f4999"));
  // Only f49990…f49999 plus "..": the stale scroll position past the end must not blank the list.
  expect(shown(rt).length).toBe(11);
  expect(rows()).toHaveLength(11);
});

test("keyboard moves focus across rows that are not rendered; the focus row is always in the DOM", async () => {
  const { rt, grid } = await setup();
  const expectFocusRow = (index: number) => {
    expect(focus(rt)).toBe(index);
    expect(grid).toHaveAttribute("aria-activedescendant", rowId(0, index));
    const row = document.getElementById(rowId(0, index));
    expect(row).not.toBeNull();
    expect(row).toHaveClass("cur");
    expect(rows().length).toBeLessThan(LIMIT);
  };

  settle(grid, () => rt.controller.command("end"));
  expectFocusRow(N);
  expect(scrolledTo.at(-1)).toBe(rowId(0, N));
  expect(document.getElementById(rowId(0, N))!.style.position).toBe(""); // scrolled into the window

  settle(grid, () => rt.controller.command("home"));
  expectFocusRow(0);
  settle(grid, () => rt.controller.command("pageDown"));
  expectFocusRow(10); // default page size without layout
  settle(grid, () => rt.controller.command("moveDown"));
  expectFocusRow(11);
  settle(grid, () => rt.controller.command("end"));
  settle(grid, () => rt.controller.command("pageUp"));
  expectFocusRow(N - 10);
  settle(grid, () => rt.controller.command("moveUp"));
  expectFocusRow(N - 11);

  // Wheel away from the focus row: it stays in the DOM (on its own), aria-activedescendant stays valid.
  scrollTo(grid, 100);
  const far = document.getElementById(rowId(0, N - 11))!;
  expect(far).not.toBeNull();
  expect(far.style.position).toBe("absolute");
  expect(Math.round(parseFloat(far.style.top) / rowHeights.normal)).toBe(N - 11); // jsdom: single precision
  expect(rows().length).toBeLessThan(LIMIT);
  // Scroll position is left alone by re-renders that do not move the focus.
  // (A reload delivers a new entries array with the same content.)
  const before = scrolledTo.length;
  act(() => rt.store.dispatch({ type: "listing", w: 0, path: activeTabOf(rt.store.getState(), 0).path, entries: bigListing() }));
  expect(scrolledTo.length).toBe(before);
  expect(scrollTops.get(grid)).toBe(100 * rowHeights.normal);
});

test("Shift range and Shift+End select over unrendered rows; selection survives window changes", async () => {
  const { rt, grid } = await setup();
  act(() => rt.controller.clickRow(0, 5, { ctrl: false, shift: false }));
  scrollTo(grid, 40_000);
  act(() => rt.controller.clickRow(0, 40_000, { ctrl: false, shift: true }));
  const selected = () => activeTabOf(rt.store.getState(), 0).selection.selected;
  expect(selected().size).toBe(40_000 - 5 + 1);
  expect(document.getElementById(rowId(0, 39_990))).toHaveAttribute("aria-selected", "true");
  expect(document.getElementById(rowId(0, 40_010))).toHaveAttribute("aria-selected", "false");

  scrollTo(grid, 20_000);
  expect(document.getElementById(rowId(0, 20_000))).toHaveAttribute("aria-selected", "true");
  scrollTo(grid, 0);
  expect(document.getElementById(rowId(0, 4))).toHaveAttribute("aria-selected", "false");
  expect(document.getElementById(rowId(0, 5))).toHaveAttribute("aria-selected", "true");

  // Ctrl click far away adds to the selection.
  scrollTo(grid, 45_000);
  act(() => rt.controller.clickRow(0, 45_000, { ctrl: true, shift: false }));
  expect(selected().size).toBe(40_000 - 5 + 2);

  settle(grid, () => rt.controller.command("home"));
  settle(grid, () => rt.controller.command("extendEnd"));
  expect(selected().size).toBe(N); // everything except ".."
  expect(rows().length).toBeLessThan(LIMIT);
});

test("type-ahead jumps to an unrendered entry and scrolls it into view", async () => {
  const { rt, grid } = await setup();
  for (const char of "f4321") settle(grid, () => rt.controller.typeahead(char));
  const target = shown(rt).findIndex((e) => e.name.startsWith("f4321"));
  expect(target).toBeGreaterThan(40_000);
  expect(focus(rt)).toBe(target);
  expect(scrolledTo.at(-1)).toBe(rowId(0, target));
  const row = document.getElementById(rowId(0, target))!;
  expect(row.style.position).toBe("");
  expect(row).toHaveClass("cur");
});

test("row height follows the density setting", async () => {
  const { rt, grid } = await setup({ density: "compact" });
  const top = grid.querySelector<HTMLElement>(".row-spacer")!;
  act(() => { grid.scrollTop = 1000 * rowHeights.compact; fireEvent.scroll(grid); });
  expect(parseFloat(top.style.height)).toBe((1000 - OVERSCAN_ROWS) * rowHeights.compact);
  expect(document.getElementById(rowId(0, 1000))).not.toBeNull();
  expect(rows().length).toBeLessThan(LIMIT);
  void rt;
});
