import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { DOCS, DOWNLOADS, setupApp } from "./test-utils";
import { newSshProfile } from "./components/dialogs/ConnectionProfileForm";

test.each(["de", "en"] as const)("tools menu keyboard, wrap, activation and focus return (%s)", async language => {
  const app = await setupApp({ language });
  const trigger = screen.getByRole("button", { name: language === "de" ? "Werkzeuge" : "Tools" });
  act(() => trigger.focus());
  await app.user.keyboard("{ArrowDown}");
  const items = screen.getAllByRole("menuitem");
  expect(items).toHaveLength(6);
  expect(items[0]).toHaveFocus();
  expect(trigger).toHaveAttribute("aria-expanded", "true");
  await app.user.keyboard("{ArrowUp}");
  expect(items[5]).toHaveFocus();
  await app.user.keyboard("{Home}{ArrowDown}");
  expect(items[1]).toHaveFocus();
  await app.user.keyboard("{End}{Enter}");
  expect(screen.getByRole("dialog", { name: language === "de" ? "Verlauf" : "History" })).toBeVisible();
  await app.user.keyboard("{Escape}");
  expect(trigger).toHaveFocus();
  await app.user.keyboard("{ArrowUp}");
  expect(screen.getAllByRole("menuitem")[5]).toHaveFocus();
  await app.user.keyboard("{Escape}");
  expect(trigger).toHaveFocus();
  expect(trigger).toHaveAttribute("aria-expanded", "false");
});

test("panel pin toggles its own folder; empty bookmarks disappear; filter shortcut and panel switch", async () => {
  const app = await setupApp({ left: DOCS, right: DOWNLOADS });
  expect(screen.queryByRole("toolbar", { name: "Lesezeichen" })).toBeNull();
  expect(app.pane(0)).toHaveClass("active");
  expect(app.pane(1)).not.toHaveClass("active");
  await app.user.keyboard("{Tab}/");
  expect(app.pane(1)).toHaveClass("active");
  const filter = within(app.pane(1)).getByRole("searchbox");
  expect(filter).toHaveFocus();
  await app.user.type(filter, "abc");
  await app.user.click(within(app.pane(1)).getByRole("button", { name: "Filter löschen" }));
  expect(filter).toHaveValue("");
  const pin = within(app.pane(0)).getByRole("button", { name: "Ordner merken …" });
  expect(pin).toHaveAttribute("aria-pressed", "false");
  await app.user.click(pin);
  expect(pin).toHaveAttribute("aria-pressed", "true");
  expect(app.runtime.store.getState().settings.bookmarks.map(b => b.path)).toEqual([DOCS]);
  expect(screen.getByRole("toolbar", { name: "Lesezeichen" })).toBeVisible();
  await app.user.click(pin);
  expect(pin).toHaveAttribute("aria-pressed", "false");
  expect(screen.queryByRole("toolbar", { name: "Lesezeichen" })).toBeNull();
});

test("connection chip empty, disconnected, connected and targeted dialog entries", async () => {
  const app = await setupApp();
  const empty = screen.getByRole("button", { name: "Verbindungen …" });
  expect(empty.querySelector(".connection-dot")).toBeNull();
  await app.user.click(empty);
  await app.user.click(screen.getByRole("menuitem", { name: "Neue Verbindung…" }));
  expect(screen.getByRole("dialog", { name: "Verbindungsprofil" })).toBeVisible();
  await app.user.keyboard("{Escape}");
  await app.user.keyboard("{Escape}");
  const saved = await app.api.connectionSave(newSshProfile("nas"));
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  const chip = screen.getByRole("button", { name: "nas · SFTP · getrennt" });
  expect(chip.querySelector(".connection-dot")).toHaveClass("disconnected");
  await app.user.click(chip);
  await app.user.click(screen.getByRole("menuitem", { name: "nas · SFTP · getrennt" }));
  await app.user.click(screen.getByRole("menuitem", { name: "nas — Verbinden" }));
  const live = await screen.findByRole("button", { name: "nas · SFTP · verbunden" });
  expect(live.querySelector(".connection-dot")).toHaveClass("connected");
  await app.user.click(live);
  await app.user.click(screen.getByRole("menuitem", { name: "nas · SFTP · verbunden" }));
  await app.user.click(screen.getByRole("menuitem", { name: /nas.*Bearbeiten/ }));
  expect(screen.getByRole("dialog", { name: "Verbindungsprofil" })).toBeVisible();
  expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(saved.profile.name);
});


test("connection import entry and Ctrl+N keep the existing manager", async () => {
  const app = await setupApp();
  await app.user.click(screen.getByRole("button", { name: "Verbindungen …" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Aus ~/.ssh/config und known_hosts importieren…" }));
  expect(await screen.findByRole("dialog", { name: "Aus ~/.ssh/config und known_hosts importieren…" })).toBeVisible();
  expect(await screen.findByRole("checkbox", { name: "nas" })).toBeVisible();
  await app.user.keyboard("{Escape}{Escape}");
  await app.user.keyboard("{Control>}n{/Control}");
  expect(screen.getByRole("dialog", { name: "Verbindungen …" })).toBeVisible();
});

test.each(["de", "en"] as const)("chip disconnect only disconnects the chosen profile (%s)", async language => {
  const app = await setupApp({ language });
  const first = await app.api.connectionSave(newSshProfile("nas"));
  const second = await app.api.connectionSave(newSshProfile("backup-box"));
  await act(async () => {
    await app.api.connectionConnect(first.profile.id);
    await app.api.connectionConnect(second.profile.id);
    await app.runtime.controller.refreshConnections();
  });
  const disconnect = vi.spyOn(app.api, "connectionDisconnect");
  const openDialog = vi.spyOn(app.runtime.controller, "openDialog");
  await app.user.click(screen.getByRole("button", { name: /nas · SFTP/ }));
  await app.user.click(screen.getByRole("menuitem", { name: /^nas · SFTP/ }));
  await app.user.click(screen.getByRole("menuitem", { name: language === "de" ? /nas.*Trennen/ : /nas.*Disconnect/ }));
  await waitFor(() => expect(app.runtime.store.getState().connections.find(e => e.profile.id === first.profile.id)?.connected).toBe(false));
  expect(disconnect).toHaveBeenCalledExactlyOnceWith(first.profile.id);
  expect(app.runtime.store.getState().connections.find(e => e.profile.id === second.profile.id)?.connected).toBe(true);
  expect(openDialog).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.queryByRole("menu")).toBeNull();
});

test.each(["de", "en"] as const)("connection rows open a submenu and chip tooltip follows menu state (%s)", async language => {
  const de = language === "de";
  const app = await setupApp({ language });
  await app.api.connectionSave(newSshProfile("nas"));
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  const chip = screen.getByRole("button", { name: /nas · SFTP/ });
  const title = chip.getAttribute("title");
  expect(title).toContain("nas");
  act(() => chip.focus());
  await app.user.keyboard("{ArrowDown}");
  expect(chip).not.toHaveAttribute("title");
  const row = screen.getByRole("menuitem", { name: de ? "nas · SFTP · getrennt" : "nas · SFTP · disconnected" });
  expect(row).toHaveFocus();
  expect(row).toHaveAttribute("aria-haspopup", "menu");
  expect(row).toHaveAttribute("aria-expanded", "false");
  expect(row.querySelector(".connection-dot")).toHaveClass("disconnected");
  expect(row).toHaveTextContent(de ? /nas\s*SFTP · getrennt/ : /nas\s*SFTP · disconnected/);
  expect(screen.queryByRole("menuitem", { name: /nas —/ })).toBeNull();
  await app.user.keyboard("{ArrowRight}");
  expect(row).toHaveAttribute("aria-expanded", "true");
  const connect = screen.getByRole("menuitem", { name: de ? "nas — Verbinden" : "nas — Connect" });
  const edit = screen.getByRole("menuitem", { name: de ? /nas.*Bearbeiten/ : /nas.*Edit/ });
  expect(connect).not.toHaveTextContent("nas");
  expect(connect).toHaveFocus();
  await app.user.keyboard("{ArrowDown}");
  expect(edit).toHaveFocus();
  await app.user.keyboard("{ArrowLeft}");
  expect(row).toHaveFocus();
  expect(screen.queryByRole("menuitem", { name: /nas —/ })).toBeNull();
  expect(row).toHaveAttribute("aria-expanded", "false");
  await app.user.keyboard("{Enter}");
  expect(screen.getByRole("menuitem", { name: de ? "nas — Verbinden" : "nas — Connect" })).toBeVisible();
  await app.user.keyboard("{Escape}");
  expect(row).toHaveFocus();
  expect(screen.getByRole("menu", { name: /nas · SFTP/  })).toBeVisible();
  await app.user.keyboard("{Escape}");
  expect(chip).toHaveFocus();
  expect(chip).toHaveAttribute("title", title);
  expect(screen.queryByRole("menu")).toBeNull();
  await app.user.click(chip);
  expect(chip).not.toHaveAttribute("title");
  await app.user.click(app.pane(0));
  expect(screen.queryByRole("menu")).toBeNull();
  expect(chip).toHaveAttribute("title", title);
});

test("connection row click toggles its submenu, only one is open and an action closes the menu", async () => {
  const app = await setupApp();
  const a = await app.api.connectionSave(newSshProfile("nas"));
  await app.api.connectionSave(newSshProfile("backup-box"));
  await act(async () => { await app.api.connectionConnect(a.profile.id); await app.runtime.controller.refreshConnections(); });
  await app.user.click(screen.getByRole("button", { name: /nas · SFTP/ }));
  const nas = screen.getByRole("menuitem", { name: "nas · SFTP · verbunden" });
  const box = screen.getByRole("menuitem", { name: "backup-box · SFTP · getrennt" });
  await app.user.click(nas);
  expect(screen.getByRole("menuitem", { name: "nas — Trennen" })).toHaveFocus();
  await app.user.click(box);
  expect(screen.queryByRole("menuitem", { name: "nas — Trennen" })).toBeNull();
  expect(screen.getByRole("menuitem", { name: "backup-box — Verbinden" })).toBeVisible();
  await app.user.click(box);
  expect(screen.queryByRole("menuitem", { name: "backup-box — Verbinden" })).toBeNull();
  await app.user.click(nas);
  const disconnect = vi.spyOn(app.api, "connectionDisconnect");
  await app.user.click(screen.getByRole("menuitem", { name: "nas — Trennen" }));
  expect(disconnect).toHaveBeenCalledWith(a.profile.id);
  expect(screen.queryByRole("menu")).toBeNull();
});

test("clicking outside closes the menu and an open submenu", async () => {
  const app = await setupApp();
  await app.api.connectionSave(newSshProfile("nas"));
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  await app.user.click(screen.getByRole("button", { name: /nas · SFTP/ }));
  await app.user.click(screen.getByRole("menuitem", { name: "nas · SFTP · getrennt" }));
  expect(screen.getAllByRole("menu")).toHaveLength(2);
  await app.user.click(app.pane(0));
  expect(screen.queryByRole("menu")).toBeNull();
});

test("chip disconnect failure reports an error without opening the manager", async () => {
  const app = await setupApp();
  const saved = await app.api.connectionSave(newSshProfile("nas"));
  await act(async () => {
    await app.api.connectionConnect(saved.profile.id);
    await app.runtime.controller.refreshConnections();
  });
  vi.spyOn(app.api, "connectionDisconnect").mockRejectedValueOnce(new Error("disconnect failed"));
  const openDialog = vi.spyOn(app.runtime.controller, "openDialog");
  const chip = screen.getByRole("button", { name: "nas · SFTP · verbunden" });
  act(() => chip.focus());
  await app.user.keyboard("{ArrowDown}{ArrowRight}{Enter}");
  await waitFor(() => expect(app.runtime.store.getState().toasts.some(t => t.kind === "error")).toBe(true));
  expect(openDialog).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(chip).toHaveFocus();
  expect(app.runtime.store.getState().connections[0].connected).toBe(true);
});

test("remote pane offers a way back to the local computer", async () => {
  const app = await setupApp();
  expect(screen.queryByRole("button", { name: "Zum lokalen Rechner" })).toBeNull();
  await app.api.connectionSave(newSshProfile("nas"));
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  await app.user.click(screen.getByRole("button", { name: "nas · SFTP · getrennt" }));
  await app.user.click(screen.getByRole("menuitem", { name: "nas · SFTP · getrennt" }));
  await app.user.click(screen.getByRole("menuitem", { name: "nas — Verbinden" }));
  await app.user.click(await within(app.pane(0)).findByRole("button", { name: /Ort wählen: nas/ }));
  await app.user.click(screen.getByRole("menuitem", { name: "Dieser Computer" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Zum lokalen Rechner" })).toBeNull());
  await app.user.click(within(app.pane(0)).getByRole("button", { name: /Ort wählen:/ }));
  await app.user.click(screen.getByRole("menuitem", { name: /nas · SFTP/ }));
  expect(await within(app.pane(0)).findByRole("button", { name: /Ort wählen: nas/ })).toBeVisible();
  expect(screen.queryByRole("button", { name: /Zum lokalen Rechner|Zurück zur Verbindung/ })).toBeNull();
});

test("place menu reconnects a disconnected profile", async () => {
  const app = await setupApp();
  const saved = await app.api.connectionSave(newSshProfile("nas"));
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  await app.user.click(screen.getByRole("button", { name: "nas · SFTP · getrennt" }));
  await app.user.click(screen.getByRole("menuitem", { name: "nas · SFTP · getrennt" }));
  await app.user.click(screen.getByRole("menuitem", { name: "nas — Verbinden" }));
  await app.user.click(await within(app.pane(0)).findByRole("button", { name: /Ort wählen: nas/ }));
  await app.user.click(screen.getByRole("menuitem", { name: "Dieser Computer" }));
  await app.api.connectionDisconnect(saved.profile.id);
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  await app.user.click(within(app.pane(0)).getByRole("button", { name: /Ort wählen:/ }));
  await app.user.click(screen.getByRole("menuitem", { name: /nas · SFTP/ }));
  expect(await within(app.pane(0)).findByRole("button", { name: /Ort wählen: nas/ })).toBeVisible();
  expect(screen.queryByRole("button", { name: /Zum lokalen Rechner|Zurück zur Verbindung/ })).toBeNull();
});
