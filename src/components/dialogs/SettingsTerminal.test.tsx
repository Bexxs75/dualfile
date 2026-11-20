import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../../app/context";
import { createRuntime, type AppRuntime } from "../../app/runtime";
import { defaultSettings } from "../../app/state";
import { createMockApi } from "../../ipc/mock";
import type { Settings } from "../../ipc/types";
import { ManualScheduler } from "../../test-utils";
import { SettingsDialog } from "./SettingsDialog";

const runtimes: AppRuntime[] = [];
afterEach(() => runtimes.splice(0).forEach(r => r.controller.dispose()));
async function setup(initial: Settings = { ...defaultSettings, language: "de" }) {
  const api = createMockApi({ scheduler: new ManualScheduler(), initialSettings: initial });
  const rt = createRuntime(api, { saveDelayMs: 1 });
  runtimes.push(rt);
  await rt.controller.start();
  render(<RuntimeProvider runtime={rt}><SettingsDialog /></RuntimeProvider>);
  await userEvent.setup().click(screen.getByRole("tab", { name: /^Terminal(?: geändert)?$/ }));
  return { api, rt, user: userEvent.setup(), save: vi.spyOn(api, "saveSettings") };
}
const contrast = () => screen.getAllByRole("status").find(e => e.textContent?.includes(": 1"))!;
const tile = (name: string) => screen.getByRole("button", { name });

test("default is green on black", async () => {
  await setup();
  expect(tile("Grün auf Schwarz")).toHaveAttribute("aria-pressed", "true");
  expect(tile("Klassisch")).toHaveAttribute("aria-pressed", "false");
  expect(contrast()).toHaveTextContent("Gut lesbar.");
});

test("settings without the field (older file) also show green", async () => {
  const { terminalColors: _c, terminalBg: _b, terminalFg: _f, ...old } = { ...defaultSettings, language: "de" as const };
  await setup(old);
  expect(tile("Grün auf Schwarz")).toHaveAttribute("aria-pressed", "true");
});

test("choosing a preset is applied and saved", async () => {
  const { rt, user, save } = await setup();
  await user.click(tile("Bernstein"));
  expect(rt.store.getState().settings.terminalColors).toBe("amber");
  expect(tile("Bernstein")).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByText("Gilt für alle Terminals, unabhängig vom Design.")).toBeInTheDocument();
  await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ terminalColors: "amber" })));
});

test("custom colors: valid hex is applied, invalid hex is flagged and not applied, swap works", async () => {
  const { rt, user } = await setup();
  expect(screen.queryByLabelText("Hintergrund als Hex-Wert")).toBeNull();
  await user.click(tile("Eigene…"));
  const bg = screen.getByLabelText("Hintergrund als Hex-Wert");
  const fg = screen.getByLabelText("Schrift als Hex-Wert");
  await user.clear(bg);
  await user.type(bg, "#12345");
  expect(bg).toHaveAttribute("aria-invalid", "true");
  expect(rt.store.getState().settings.terminalBg).toBe("#102030");
  await user.type(bg, "6");
  expect(bg).toHaveAttribute("aria-invalid", "false");
  expect(rt.store.getState().settings.terminalBg).toBe("#123456");
  await user.clear(fg);
  await user.type(fg, "FFFFFF");
  expect(rt.store.getState().settings.terminalFg).toBe("#ffffff");
  await user.click(screen.getByRole("button", { name: "Farben tauschen" }));
  expect(rt.store.getState().settings).toMatchObject({ terminalBg: "#ffffff", terminalFg: "#123456" });
  expect(bg).toHaveValue("#ffffff");
});

test("low contrast shows a hint but the colors are still applied", async () => {
  const { rt, user } = await setup({ ...defaultSettings, language: "de", terminalColors: "custom", terminalBg: "#777777", terminalFg: "#808080" });
  expect(contrast()).toHaveTextContent("Wenig Kontrast");
  expect(contrast()).toHaveTextContent("trotzdem übernommen");
  await user.click(tile("Hell"));
  expect(rt.store.getState().settings.terminalColors).toBe("light");
  expect(contrast()).toHaveTextContent("Gut lesbar.");
  expect(rt.store.getState().settings.terminalBg).toBe("#777777");
});
