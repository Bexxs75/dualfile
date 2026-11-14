/// <reference types="vite/client" />
import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { setupApp } from "./test-utils";
import { newSshProfile } from "./components/dialogs/ConnectionProfileForm";
import { activeTabOf } from "./app/state";
import { crumbsOf, displayPath, parentPath } from "./state/paths";
import { contrastRatio, themes, themeNames } from "./themes/tokens";
import css from "./components/dialogs/ssh.css?raw";

test.each(["de", "en"] as const)("remote badges, crumbs and connection count follow dialog connect/disconnect (%s)", async language => {
  const app = await setupApp({ language });
  await act(async () => { await app.api.connectionSave({ ...newSshProfile("nas"), name: "SSH Home", user: "demo", port: 2222 }); });
  await app.user.click(screen.getByRole("button", { name: language === "de" ? "Verbindungen …" : "Connections …" }));
  await app.user.click(screen.getByRole("menuitem", { name: language === "de" ? "Alle Verbindungen verwalten …" : "Manage all connections …" }));
  await app.user.click(await screen.findByRole("button", { name: language === "de" ? "Verbinden" : "Connect" }));
  const badge = await within(app.pane(0)).findByRole("button", { name: /(?:Ort wählen|Choose location): SSH Home · SFTP/ });
  expect(badge).toHaveAttribute("title", "SSH Home · SFTP · demo@nas.example:2222");
  expect(badge.querySelector(".connection-dot")).toHaveClass("connected");
  expect(within(app.pane(0)).getByRole("status")).toHaveTextContent(language === "de" ? "Fernzugriff" : "Remote access");
  const root = activeTabOf(app.runtime.store.getState(), 0).path;
  expect(root).toMatch(/^sftp:[^!]+!\//);
  await app.user.click(within(app.pane(0)).getByRole("button", { name: "demo@nas" }));
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(root);
  const indicator = screen.getByRole("button", { name: language === "de" ? "SSH Home · SFTP · verbunden" : "SSH Home · SFTP · connected" });
  expect(indicator).toHaveAttribute("title", expect.stringContaining("SSH Home"));
  await app.user.click(indicator);
  await app.user.click(screen.getByRole("menuitem", { name: language === "de" ? /SSH Home.*Trennen/ : /SSH Home.*Disconnect/ }));
  await waitFor(() => expect(screen.getByRole("button", { name: language === "de" ? "SSH Home · SFTP · getrennt" : "SSH Home · SFTP · disconnected" })).toBeVisible());
  expect(badge.querySelector(".connection-dot")).toHaveClass("disconnected");
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(root);
});

test("listing failure refreshes status, retains old session identity across reconnect and counts two profiles", async () => {
  const app = await setupApp();
  const a = await app.api.connectionSave(newSshProfile("nas"));
  const b = await app.api.connectionSave(newSshProfile("backup-box"));
  let root = "";
  await act(async () => {
    const result = await app.api.connectionConnect(a.profile.id);
    if (result.state !== "connected") throw Error(result.state);
    root = result.root;
    await app.runtime.controller.navigateTo(0, root);
    await app.api.connectionConnect(b.profile.id);
    await app.runtime.controller.refreshConnections();
  });
  await app.user.click(screen.getByRole("button", { name: "nas · SFTP · verbunden" }));
  expect(screen.getByText("backup-box · SFTP · verbunden")).toBeVisible();
  expect(screen.getByRole("menuitem", { name: /backup-box.*Trennen/ })).toBeVisible();
  await app.user.keyboard("{Escape}");
  expect(within(app.pane(0)).getByRole("button", { name: "nas" })).toBeVisible();
  await act(async () => { await app.api.connectionDisconnect(a.profile.id); await app.runtime.controller.reload(0); });
  expect(within(app.pane(0)).getByRole("button", { name: /Ort wählen: nas/ }).querySelector(".connection-dot")).toHaveClass("disconnected");
  expect(screen.getByRole("button", { name: "nas · SFTP · getrennt" })).toBeVisible();
  await app.user.click(screen.getByRole("button", { name: "nas · SFTP · getrennt" }));
  expect(screen.getByText("backup-box · SFTP · verbunden")).toBeVisible();
  expect(screen.getByRole("menuitem", { name: /backup-box.*Trennen/ })).toBeVisible();
  await app.user.keyboard("{Escape}");
  await act(async () => { await app.api.connectionConnect(a.profile.id); await app.runtime.controller.refreshConnections(); });
  expect(within(app.pane(0)).getByRole("button", { name: /Ort wählen: nas/ }).querySelector(".connection-dot")).toHaveClass("disconnected");
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(root);
  await app.user.click(screen.getByRole("button", { name: "nas · SFTP · getrennt" }));
  expect(screen.getByText("nas · SFTP · verbunden")).toBeVisible();
  expect(screen.getByRole("menuitem", { name: /nas.*Trennen/ })).toBeVisible();
  expect(screen.getByText("backup-box · SFTP · verbunden")).toBeVisible();
  expect(screen.getByRole("menuitem", { name: /backup-box.*Trennen/ })).toBeVisible();
});

test.each(["sftp", "ftp", "ftps"])("%s paths never expose session IDs and crumbs keep the connection", protocol => {
  const path = `${protocol}:session-123!/pub/a b`;
  const connections = { "session-123": { profile: { ...newSshProfile("nas"), name: "SSH Home", user: "demo" }, connected: true, endpoint: "demo@nas:22" } };
  expect(displayPath(path, connections)).toBe("demo@nas/pub/a b");
  expect(displayPath(path)).toBe(`${protocol.toUpperCase()}/pub/a b`);
  expect(crumbsOf(path, connections)).toEqual([
    { label: "demo@nas", path: `${protocol}:session-123!/` },
    { label: "pub", path: `${protocol}:session-123!/pub` },
    { label: "a b", path },
  ]);
  expect(parentPath(`${protocol}:session-123!/`)).toBe(`${protocol}:session-123!/`);
});

test.each(themeNames)("SSH hint and resolved text use theme tokens with 4.5:1 contrast in %s", name => {
  expect(css).toMatch(/\.ssh-info\s*\{[^}]*background: var\(--panel2\);[^}]*color: var\(--fg\)/);
  expect(contrastRatio(themes[name].fg, themes[name].panel2)).toBeGreaterThanOrEqual(4.5);
});
