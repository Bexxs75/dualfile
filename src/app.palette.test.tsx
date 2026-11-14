import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import { HOME, DOCS, DOWNLOADS, setupApp } from "./test-utils";
import { createCommands, matchCommand } from "./app/commands";
import { createTranslator } from "./i18n";
import { systemPlaceProfile, mockSystemPlaces } from "./ipc/systemPlaces";

const open = () => fireEvent.keyDown(document.activeElement!, { key: "k", ctrlKey: true });
const search = (value: string) => fireEvent.change(screen.getByRole("combobox"), { target: { value } });

describe("Befehlspalette R3", () => {
  test("Strg+K/P toggle, Escape returns to active panel, toolbar and ARIA/focus trap", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Tab}");
    const grid = within(app.pane(1)).getByRole("grid");
    open();
    const dialog = screen.getByRole("dialog", { name: "Befehlspalette" });
    const input = screen.getByRole("combobox");
    expect(input).toHaveFocus();
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(input).toHaveAttribute("aria-controls", screen.getByRole("listbox").id);
    expect(document.getElementById(input.getAttribute("aria-activedescendant")!)).toHaveAttribute("role", "option");
    await app.user.tab();
    expect(within(dialog).getByRole("button", { name: "Schließen" })).toHaveFocus();
    await app.user.tab();
    expect(input).toHaveFocus();
    fireEvent.keyDown(input, { key: "p", ctrlKey: true });
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(grid).toHaveFocus();
    fireEvent.keyDown(grid, { key: "p", ctrlKey: true });
    await app.user.keyboard("{Escape}");
    expect(grid).toHaveFocus();
    await app.user.click(screen.getByRole("button", { name: /Befehl, Ordner/ }));
    await app.user.keyboard("{Escape}");
    expect(grid).toHaveFocus();
  });

  test("fuzzy matching, umlauts, highlight, no results, disabled execution and controller dispatch", async () => {
    const app = await setupApp();
    const spy = vi.spyOn(app.runtime.controller, "command");
    open(); search("UMBeNENNEN");
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-disabled", "true");
    await app.user.keyboard("{Enter}");
    expect(spy).not.toHaveBeenCalled();
    await app.user.click(screen.getAllByRole("option")[0]!);
    expect(spy).not.toHaveBeenCalled();
    search("zzzzzzzzz");
    expect(screen.getByText("Kein Treffer")).toBeInTheDocument();
    expect(screen.getByRole("combobox")).not.toHaveAttribute("aria-activedescendant");
    search("verst date");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(screen.getByRole("option").querySelector("mark")).not.toBeNull();
    await app.user.keyboard("{Enter}");
    expect(spy).toHaveBeenCalledWith("toggleHidden");
    expect(app.runtime.store.getState().showHidden).toBe(true);
    expect(matchCommand("Ähnliche Bilder", "aehn bild")).not.toBeNull();
    expect(matchCommand("Tab schließen", "schliessen")).not.toBeNull();
  });

  test("dynamic bookmarks and tabs navigate using existing controller", async () => {
    const app = await setupApp({ settings: { bookmarks: [{ name: "Arbeit", path: DOCS, color: "#ffffff", order: 0 }] } });
    const bookmark = vi.spyOn(app.runtime.controller, "openBookmark");
    open(); search("Lesezeichen Arbeit");
    await app.user.keyboard("{Enter}");
    expect(bookmark).toHaveBeenCalledWith(DOCS);
    await waitFor(() => expect(app.runtime.store.getState().windows[0].tabs[0]?.path).toBe(DOCS));
    open(); search("Zu Tab");
    expect(screen.getAllByRole("option")).toHaveLength(2);
    await app.user.keyboard("{ArrowDown}{Enter}");
    expect(app.runtime.store.getState().active).toBe(1);
    expect(app.runtime.store.getState().windows[1].tabs[0]?.path).toBe(DOWNLOADS);
  });

  test("connection entries include system mounts and use connect/disconnect dialog routes", async () => {
    const app = await setupApp();
    const profile = { ...systemPlaceProfile(mockSystemPlaces[1]!), id: "system-test", name: "NAS" };
    act(() => app.runtime.store.dispatch({ type: "connections", entries: [{ profile, connected: true, connectionId: null, pins: [], root: HOME, secretSet: false, secretStore: "session" }], remote: {} }));
    const spy = vi.spyOn(app.runtime.controller, "openDialog").mockImplementation(() => {});
    open(); search("Trennen NAS");
    await app.user.keyboard("{Enter}");
    expect(spy).toHaveBeenCalledWith({ kind: "connections", entry: { action: "disconnect", profile } });
    act(() => app.runtime.store.dispatch({ type: "connections", entries: [{ profile, connected: false, connectionId: null, pins: [], root: HOME, secretSet: false, secretStore: "session" }], remote: {} }));
    open(); search("Verbinden NAS");
    await app.user.keyboard("{Enter}");
    expect(spy).toHaveBeenCalledWith({ kind: "connections", entry: { action: "connect", profile } });
    const commands = createCommands(app.runtime.store.getState(), app.runtime.controller, createTranslator("de"), vi.fn());
    commands.find(c => c.id === "system")!.execute();
    expect(spy).toHaveBeenCalledWith({ kind: "connections", entry: { action: "system" } });
  });

  test("system import and mounted connection entry reuse real dialog flows without unmounting", async () => {
    const app = await setupApp();
    open(); search("Aus System übernehmen");
    await app.user.keyboard("{Enter}");
    expect(await screen.findByRole("dialog", { name: "Netzwerkorte aus dem System" })).toBeVisible();
    fireEvent.keyDown(document.activeElement!, { key: "k", ctrlKey: true });
    expect(screen.queryByRole("combobox")).toBeNull();
    await app.user.keyboard("{Escape}{Escape}");
    await act(async () => {
      await app.api.connectionSave({ ...systemPlaceProfile(mockSystemPlaces[1]!), protocol: "system", systemUrl: "smb://nas/team", share: "", port: 0, startFolder: "/team" });
      await app.runtime.controller.refreshConnections();
    });
    const navigate = vi.spyOn(app.runtime.controller, "navigateTo").mockResolvedValue(true);
    const unmount = vi.spyOn(app.api, "systemMountUnmount");
    open(); search("Trennen Team");
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(0, HOME));
    expect(unmount).not.toHaveBeenCalled();
  });

  test("five recent commands stay first across reopening; settings receives focus", async () => {
    const app = await setupApp();
    for (const query of ["Versteckte Dateien", "Panels koppeln", "Neuer Tab", "Design Schiefer", "Design Papier", "Design Werkstatt"]) {
      open(); expect(screen.queryByRole("combobox"), query).not.toBeNull(); search(query); await app.user.keyboard("{Enter}");
    }
    open();
    const recent = screen.getByRole("group", { name: "Zuletzt benutzt" });
    expect(within(recent).getAllByRole("option")).toHaveLength(5);
    expect(within(recent).getAllByRole("option")[0]).toHaveTextContent("Werkstatt");
    expect(within(recent).queryByText("Versteckte Dateien")).toBeNull();
    search("Einstellungen"); await app.user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog", { name: /Einstellungen/ });
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(app.runtime.store.getState().windows[0].tabs[0]?.path).toBe(HOME);
  });
});
