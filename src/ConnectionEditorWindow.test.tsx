import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { ConnectionEditorWindow, parseEditorParams } from "./ConnectionEditorWindow";
import { createMockApi } from "./ipc/mock";
import { newSshProfile } from "./components/dialogs/ConnectionProfileForm";

afterEach(() => { cleanup(); vi.restoreAllMocks(); document.documentElement.className = ""; document.documentElement.removeAttribute("style"); });

const query = (extra = "") => `?view=connection-editor&theme=papier&accent=%232f6f6a&accentAutomatic=0&density=normal&fontSize=15&lang=en${extra}`;

test("parseEditorParams reads the main window's look and falls back on garbage", () => {
  expect(parseEditorParams(query("&profile=p%201"))).toEqual({
    profileId: "p 1",
    appearance: { theme: "papier", accent: "#2f6f6a", accentAutomatic: false, density: "normal", fontSize: 15, lang: "en" },
  });
  const odd = parseEditorParams("?theme=x&density=y&lang=fr&fontSize=nope");
  expect(odd.profileId).toBeNull();
  expect(odd.appearance).toMatchObject({ theme: "schiefer", density: "normal", lang: "de", fontSize: 13 });
});

test("a new profile form shows up in the window's language, focused, with the theme applied", async () => {
  const api = createMockApi();
  render(<ConnectionEditorWindow api={api} search={query()} />);
  const name = await screen.findByLabelText("Name");
  await waitFor(() => expect(name).toHaveFocus());
  expect(screen.getByRole("button", { name: "Save" })).toBeVisible();
  expect(document.documentElement.dataset.fm).toBe("papier");
  expect(document.documentElement.lang).toBe("en");
  expect(document.documentElement.classList.contains("editor-window-root")).toBe(true);
  expect(document.documentElement.style.getPropertyValue("--fs")).toBe("15px");
});

test("it never reads or writes settings and never subscribes to events", async () => {
  const api = createMockApi();
  const load = vi.spyOn(api, "loadSettings");
  const save = vi.spyOn(api, "saveSettings");
  const events = vi.spyOn(api, "onEvent");
  const undo = vi.spyOn(api, "onUndoChanged");
  render(<ConnectionEditorWindow api={api} search={query()} />);
  await screen.findByLabelText("Name");
  expect([load, save, events, undo].map((spy) => spy.mock.calls.length)).toEqual([0, 0, 0, 0]);
});

test("an existing profile is loaded by id; saving closes the window", async () => {
  const api = createMockApi();
  const saved = await api.connectionSave({ ...newSshProfile("nas"), name: "Mein NAS" });
  const close = vi.spyOn(api, "connectionEditorClose");
  const user = userEvent.setup();
  render(<ConnectionEditorWindow api={api} search={query(`&profile=${saved.profile.id}`)} />);
  const name = await screen.findByLabelText("Name");
  expect(name).toHaveValue("Mein NAS");
  await user.clear(name);
  await user.type(name, "Neues NAS");
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect((await api.connectionsList())[0]?.profile.name).toBe("Neues NAS");
});

test("a profile that no longer exists shows an error and a working Close button", async () => {
  const api = createMockApi();
  const close = vi.spyOn(api, "connectionEditorClose");
  const user = userEvent.setup();
  render(<ConnectionEditorWindow api={api} search={query("&profile=gone")} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("no longer exists");
  expect(screen.queryByLabelText("Name")).toBeNull();
  await waitFor(() => expect(screen.getByRole("button", { name: "Close" })).toHaveFocus());
  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(close).toHaveBeenCalledOnce();
});

test("Cancel and Escape close; Escape does not when something already handled it", async () => {
  const api = createMockApi();
  const close = vi.spyOn(api, "connectionEditorClose");
  const user = userEvent.setup();
  render(<ConnectionEditorWindow api={api} search={query()} />);
  await screen.findByLabelText("Name");
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(close).toHaveBeenCalledTimes(1);
  await user.keyboard("{Escape}");
  expect(close).toHaveBeenCalledTimes(2);
  const handled = new KeyboardEvent("keydown", { key: "Escape", cancelable: true, bubbles: true });
  handled.preventDefault();
  act(() => { document.dispatchEvent(handled); });
  expect(close).toHaveBeenCalledTimes(2);
});
