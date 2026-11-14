import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../../app/context";
import { createRuntime } from "../../app/runtime";
import { createMockApi } from "../../ipc/mock";
import { ConnectionProfileForm, newSshProfile } from "./ConnectionProfileForm";
import { crumbsOf, parentPath } from "../../state/paths";
import { systemPlaceProfile } from "../../ipc/systemPlaces";

afterEach(cleanup);

test("SMB form saves domain/share and explicit guest; mock connects without trust and navigates remotely", async () => {
  const api = createMockApi();
  const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language: "en" }) });
  const saved = vi.fn();
  render(<RuntimeProvider runtime={runtime}><ConnectionProfileForm api={api} initial={newSshProfile("demo-smb")} onSaved={saved} onCancel={() => {}} /></RuntimeProvider>);
  const user = userEvent.setup();
  await user.selectOptions(screen.getByLabelText("Protocol"), "smb");
  expect(screen.getByLabelText("Port")).toHaveValue(445);
  expect(screen.queryByLabelText("Transport")).toBeNull();
  await user.type(screen.getByLabelText("Share (empty: list shares)"), "team");
  await user.type(screen.getByLabelText("Domain / workgroup"), "WORKGROUP");
  await user.click(screen.getByLabelText("Sign in as guest"));
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(saved).toHaveBeenCalledOnce());
  const entry = (await api.connectionsList())[0]!;
  expect(entry.profile).toMatchObject({ protocol: "smb", port: 445, share: "team", domain: "WORKGROUP", guest: true });
  const result = await api.connectionConnect(entry.profile.id);
  expect(result.state).toBe("connected");
  if (result.state !== "connected") throw new Error("SMB must not request certificate trust");
  expect(result.root).toMatch(/^smb:/);
  await api.list(result.root);
  const folder = await api.makeDir(result.root, "new");
  expect(parentPath(folder)).toBe(result.root);
  const renamed = await api.rename(folder, "renamed");
  expect(renamed.newPath).toContain("/renamed");
  const connection = { profile: { ...entry.profile, user: "alice" }, connected: true, endpoint: "demo-smb:445" };
  expect(crumbsOf(renamed.newPath, { [result.connectionId]: connection }).map(c => c.label)).toEqual(["alice@demo-smb", "team", "renamed"]);
  await api.connectionDisconnect(entry.profile.id);
  await expect(api.list(result.root)).rejects.toMatchObject({ kind: "disconnected" });
});

test("SMB native import separates share from path, never opts into guest, validates share", async () => {
  const profile = systemPlaceProfile({ sources: ["gnome"], name: "NAS", scheme: "smb", host: "nas", port: null, user: "alice", path: "/team/docs", url: "smb://alice@nas/team/docs", mountedAt: null });
  expect(profile).toMatchObject({ protocol: "smb", port: 445, share: "team", startFolder: "/docs", user: "alice", systemUrl: null, saveSecret: false });
  expect(profile.guest ?? false).toBe(false);
  const api = createMockApi();
  for (const share of ["a/b", "a\\b", "..", "bad\n"]) await expect(api.connectionSave({ ...profile, share })).rejects.toMatchObject({ kind: "invalidInput" });
  const entry = await api.connectionSave({ ...profile, host: "demo-smb", startFolder: "/" });
  await expect(api.connectionConnect(entry.profile.id, "wrong-MARKER")).rejects.toMatchObject({ kind: "smbAuth" });
  expect(JSON.stringify(await api.connectionsList())).not.toContain("wrong-MARKER");
});


test("empty SMB share browses shares and missing shares have a distinct error", async () => {
  const api = createMockApi();
  const profile = { ...newSshProfile("demo-smb"), protocol: "smb" as const, transport: "builtin" as const, port: 445, guest: true, share: "" };
  const entry = await api.connectionSave(profile);
  const result = await api.connectionConnect(entry.profile.id);
  if (result.state !== "connected") throw new Error("not connected");
  expect((await api.list(result.root)).entries.map(e => e.name).sort()).toEqual(["public", "team"]);
  const missing = await api.connectionSave({ ...profile, name: "Missing", share: "missing" });
  await expect(api.connectionConnect(missing.profile.id)).rejects.toMatchObject({ kind: "smbShareMissing" });
});
