import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { HOME, setupApp } from "./test-utils";
import { activeTabOf } from "./app/state";
import { newSshProfile } from "./components/dialogs/ConnectionProfileForm";

async function prepare() {
  const app = await setupApp();
  const saved = await app.api.connectionSave({ ...newSshProfile("eddie"), transport: "openssh" });
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  return { app, saved };
}
const listed = (app: Awaited<ReturnType<typeof prepare>>["app"], w: 0 | 1) => ({ listing: app.runtime.store.getState().listings[w] });

test("pane place menu: not-connected openssh profile loads the remote listing", async () => {
  const { app } = await prepare();
  await app.user.click(within(app.pane(0)).getByRole("button", { name: /Ort wählen/ }));
  await app.user.click(screen.getByRole("menuitem", { name: /eddie · SFTP/ }));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toMatch(/^sftp:/));
  const path = activeTabOf(app.runtime.store.getState(), 0).path;
  await waitFor(() => expect(listed(app, 0).listing?.path).toBe(path));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("toolbar menu: reconnect of openssh profile loads the remote listing in the active pane", async () => {
  const { app } = await prepare();
  await app.user.click(screen.getByRole("button", { name: /eddie · SFTP/ }));
  await app.user.click(screen.getByRole("menuitem", { name: /^eddie · SFTP/ }));
  await app.user.click(screen.getByRole("menuitem", { name: /eddie — Verbinden/ }));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), app.runtime.store.getState().active).path).toMatch(/^sftp:/));
  const w = app.runtime.store.getState().active;
  const path = activeTabOf(app.runtime.store.getState(), w).path;
  await waitFor(() => expect(listed(app, w).listing?.path).toBe(path));
  expect(HOME).not.toBe(path);
});

test("choosing the not-connected profile twice (double-click) still loads the remote listing", async () => {
  const { app, saved } = await prepare();
  const original = app.api.connectionConnect.bind(app.api);
  let resume!: () => void;
  const started = new Promise<void>(r => { vi.spyOn(app.api, "connectionConnect").mockImplementationOnce(async (id, secret) => {
    r();
    await new Promise<void>(resolve => { resume = resolve; });
    return original(id, secret);
  }); });
  await act(async () => { void app.runtime.controller.chooseConnection(0, saved.profile); });
  await started;
  await act(async () => { void app.runtime.controller.chooseConnection(0, saved.profile); });
  await act(async () => { resume(); });
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toMatch(/^sftp:/));
  const path = activeTabOf(app.runtime.store.getState(), 0).path;
  await waitFor(() => expect(listed(app, 0).listing?.path).toBe(path));
});
