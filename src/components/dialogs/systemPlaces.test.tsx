import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { CoreError } from "../../ipc/api";
import { RuntimeProvider } from "../../app/context";
import { createRuntime } from "../../app/runtime";
import { createMockApi } from "../../ipc/mock";
import { ConnectionsDialog } from "./ConnectionsDialog";
import { SystemPlacesDialog } from "./SystemPlacesDialog";
import { systemPlaceProfile, mockSystemPlaces } from "../../ipc/systemPlaces";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
function setup(api = createMockApi()) {
  const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language: "de" }) });
  render(<RuntimeProvider runtime={runtime}><ConnectionsDialog api={api} onClose={() => {}} /></RuntimeProvider>);
  return { api, runtime, user: userEvent.setup() };
}
test("empty profiles hint, source labels, multi-selection and duplicate imports", async () => {
  const { api, user } = setup();
  expect(await screen.findByText(/Im System wurden Netzwerkorte/)).toBeVisible();
  await user.click(screen.getByRole("button", { name: "Aus System übernehmen …" }));
  expect(await screen.findByRole("dialog", { name: "Netzwerkorte aus dem System" })).toBeVisible();
  expect(screen.getByText(/Dolphin · GNOME Dateien/)).toBeVisible();
  for (const input of screen.getAllByRole("checkbox")) await user.click(input);
  await user.click(screen.getByRole("button", { name: "Übernehmen" }));
  await waitFor(async () => expect(await api.connectionsList()).toHaveLength(2));
  expect((await api.connectionsList()).map(e => e.profile.protocol)).toEqual(["sftp", "smb"]);
  await user.click(screen.getByRole("button", { name: "Aus System übernehmen …" }));
  await screen.findByRole("dialog", { name: "Netzwerkorte aus dem System" });
  for (const input of screen.getAllByRole("checkbox")) expect(input).toBeDisabled();
  expect(screen.getAllByText(/schon vorhanden/)).toHaveLength(2);
  expect(screen.getByRole("button", { name: "Übernehmen" })).toBeDisabled();
});
test("system connect navigates locally without asking for a password; disconnect never unmounts", async () => {
  const api = createMockApi();
  await api.connectionSave({ ...systemPlaceProfile(mockSystemPlaces[1]!), protocol: "system", systemUrl: "smb://nas/team", share: "", port: 0, startFolder: "/team" });
  const mount = vi.spyOn(api, "systemMountConnect");
  const unmount = vi.spyOn(api, "systemMountUnmount");
  const native = vi.spyOn(api, "connectionConnect");
  const { runtime, user } = setup(api);
  const navigate = vi.spyOn(runtime.controller, "navigateTo").mockResolvedValue(true);
  await user.click(await screen.findByRole("button", { name: "Verbinden" }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith(0, "/home/demo"));
  expect(mount).toHaveBeenCalledOnce(); expect(native).not.toHaveBeenCalled();
  expect(screen.queryByLabelText("Antwort")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Trennen" }));
  await waitFor(() => expect(navigate).toHaveBeenCalledTimes(2));
  expect(unmount).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Aushängen" }));
  await waitFor(() => expect(unmount).toHaveBeenCalledOnce());
});
test("empty discovery explains sources and existing targets are matched despite names", async () => {
  const api = createMockApi(); const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language: "de" }) });
  const view = render(<RuntimeProvider runtime={runtime}><SystemPlacesDialog api={api} places={[]} profiles={[]} onClose={() => {}} onImported={async () => {}} /></RuntimeProvider>);
  expect(screen.getByText(/GTK-3- und GTK-Lesezeichen/)).toBeVisible();
  const profile = { ...systemPlaceProfile(mockSystemPlaces[0]!), name: "Different", port: 22 };
  view.rerender(<RuntimeProvider runtime={runtime}><SystemPlacesDialog api={api} places={mockSystemPlaces} profiles={[profile]} onClose={() => {}} onImported={async () => {}} /></RuntimeProvider>);
  expect(screen.getAllByRole("checkbox")[0]).toBeDisabled();
  expect(screen.getAllByRole("checkbox")[1]).not.toBeDisabled();
});

test("unmounted system profiles offer only edit, connect and delete actions", async () => {
  const api = createMockApi();
  const entry = await api.connectionSave({ ...systemPlaceProfile(mockSystemPlaces[1]!), protocol: "system", systemUrl: "smb://nas/team", share: "", port: 0, startFolder: "/team" });
  await api.systemMountUnmount(entry!.profile.id);
  const { user } = setup(api);
  await screen.findByRole("button", { name: "Verbinden" });
  expect(screen.queryByRole("button", { name: "Trennen" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Aushängen" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Verbinden" }));
  expect(await screen.findByRole("button", { name: "Aushängen" })).toBeVisible();
  expect(screen.getAllByRole("button", { name: "Trennen" })).toHaveLength(1);
  await user.click(screen.getByRole("button", { name: "Aushängen" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Aushängen" })).toBeNull());
  expect(screen.queryByRole("button", { name: "Trennen" })).toBeNull();
});

test.each(["de", "en"] as const)("mount failure displays diagnostics in %s, not the missing-helper hint", async language => {
  const api = createMockApi();
  await api.connectionSave({ ...systemPlaceProfile(mockSystemPlaces[1]!), protocol: "system", systemUrl: "smb://nas/team", share: "", port: 0, startFolder: "/team" });
  vi.spyOn(api, "systemMountConnect").mockRejectedValue(new CoreError("Host unreachable", "systemMountFailed"));
  const { user, runtime } = setup(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language }) });
  await user.click(await screen.findByRole("button", { name: language === "de" ? "Verbinden" : "Connect" }));
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent(language === "de" ? "Der Ort konnte nicht eingehängt werden: Host unreachable" : "The location could not be mounted: Host unreachable");
  expect(alert).not.toHaveTextContent("GVfs");
});
