import { expect, test, vi } from "vitest";
import { createRuntime } from "./runtime";
import { createMockApi } from "../ipc/mock";
import type { CoreEvent } from "../ipc/types";
import { newSshProfile } from "../components/dialogs/ConnectionProfileForm";

function setup() {
  const base = createMockApi();
  let emit!: (event: CoreEvent) => void;
  const api = { ...base, onEvent: (listener: (event: CoreEvent) => void) => { emit = listener; return () => {}; } };
  const runtime = createRuntime(api);
  return { ...runtime, base, api, emit: (event: CoreEvent) => emit(event) };
}

test("connectionsChanged (a save in the editor window) refreshes the connection list", async () => {
  const r = setup();
  await r.controller.start();
  expect(r.store.getState().connections).toHaveLength(0);
  await r.base.connectionSave({ ...newSshProfile("nas"), name: "NAS" });
  r.emit({ type: "connectionsChanged" });
  await vi.waitFor(() => expect(r.store.getState().connections.map((e) => e.profile.name)).toEqual(["NAS"]));
  r.controller.dispose();
});

test("openConnectionEditor sends the look of this window and reports an already open editor", async () => {
  const r = setup();
  r.store.dispatch({ type: "settings", update: (s) => ({ ...s, language: "en", theme: "papier", accent: "#112233", accentAutomatic: true, density: "compact", fontSize: 15 }) });
  const open = vi.spyOn(r.api, "connectionEditorOpen").mockResolvedValueOnce(true).mockResolvedValueOnce(false);
  const profile = { ...newSshProfile("nas"), id: "p-9" };
  await r.controller.openConnectionEditor(profile);
  expect(open).toHaveBeenLastCalledWith("p-9", "Connection profile", { theme: "papier", accent: "#112233", accentAutomatic: true, density: "compact", fontSize: 15, lang: "en" });
  expect(r.store.getState().toasts).toHaveLength(0);
  await r.controller.openConnectionEditor();
  expect(open).toHaveBeenLastCalledWith(null, "Connection profile", expect.anything());
  expect(r.store.getState().toasts.map((t) => t.text)).toEqual([expect.stringContaining("already being edited")]);
});
