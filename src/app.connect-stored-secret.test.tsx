import { act, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { setupApp } from "./test-utils";
import { newSshProfile } from "./components/dialogs/ConnectionProfileForm";
import { CoreError } from "./ipc/api";
import { DEMO_PASSWORD } from "./ipc/mock";
import type { ConnectionProfile } from "./ipc/types";

const passwordField = () => document.querySelector('input[type="password"]');

async function open(withSecret: boolean) {
  const app = await setupApp();
  const saved = await app.api.connectionSave({ ...newSshProfile("nas"), transport: "builtin", port: 22, user: "demo", auth: { kind: "password" }, saveSecret: true });
  if (withSecret) await app.api.secretSet(saved.profile.id, DEMO_PASSWORD);
  await act(async () => { await app.runtime.controller.refreshConnections(); });
  const profile = app.runtime.store.getState().connections.find(e => e.profile.id === saved.profile.id)!.profile;
  return { app, profile };
}
const show = (app: Awaited<ReturnType<typeof open>>["app"], profile: ConnectionProfile) =>
  act(() => app.runtime.store.dispatch({ type: "dialog", dialog: { kind: "connections", entry: { action: "connect", profile } } }));

test("stored secret: connect without a password dialog (#48)", async () => {
  const { app, profile } = await open(true);
  const connect = vi.spyOn(app.api, "connectionConnect");
  show(app, profile);
  await waitFor(() => expect(connect).toHaveBeenCalled());
  expect(connect.mock.calls[0][1]).toBeUndefined();
  expect(passwordField()).toBeNull();
});

test("no stored secret: password dialog first", async () => {
  const { app, profile } = await open(false);
  const connect = vi.spyOn(app.api, "connectionConnect");
  show(app, profile);
  await waitFor(() => expect(passwordField()).not.toBeNull());
  expect(connect).not.toHaveBeenCalled();
});

test("stored secret rejected by the server: password dialog follows", async () => {
  const { app, profile } = await open(true);
  const connect = vi.spyOn(app.api, "connectionConnect").mockRejectedValueOnce(new CoreError("auth", "auth"));
  show(app, profile);
  await waitFor(() => expect(passwordField()).not.toBeNull());
  expect(connect).toHaveBeenCalledTimes(1);
});
