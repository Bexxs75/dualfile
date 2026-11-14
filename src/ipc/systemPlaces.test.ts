import { expect, test } from "vitest";
import { createMockApi } from "./mock";
import { mockSystemPlaces, systemPlaceProfile } from "./systemPlaces";

test("mock import is idempotent, survives name collisions and rejects arbitrary URLs", async () => {
  const api = createMockApi();
  await api.connectionSave({ ...systemPlaceProfile(mockSystemPlaces[0]!), host: "other" });
  await api.systemPlacesImport(mockSystemPlaces.map(p => p.url));
  await api.systemPlacesImport(mockSystemPlaces.map(p => p.url));
  const entries = await api.connectionsList();
  expect(entries).toHaveLength(3);
  expect(entries[1]?.profile.name).toBe("Home server (2)");
  expect(entries.every(e => !e.profile.saveSecret)).toBe(true);
  await expect(api.systemPlacesImport(["smb://u:secret@host/share"])).rejects.toMatchObject({ kind: "invalidInput" });
  expect(entries.find(e => e.profile.protocol === "smb")?.profile).toMatchObject({ host: "nas", share: "team", port: 445, startFolder: "/", guest: false });
  const system = await api.connectionSave({ ...systemPlaceProfile(mockSystemPlaces[1]!), name: "Legacy share", protocol: "system", systemUrl: "smb://nas/team", share: "", port: 0, startFolder: "/team" });
  await expect(api.secretSet(system.profile.id, "secret")).rejects.toMatchObject({ kind: "invalidInput" });
  await expect(api.connectionSave({ ...system.profile, systemUrl: "smb://u:secret@nas/team" })).rejects.toMatchObject({ kind: "invalidInput" });
  expect(await api.systemMountConnect(system.profile.id)).toBe("/home/demo");
  expect((await api.connectionsList()).find(e => e.profile.id === system.profile.id)?.connected).toBe(true);
  await api.systemMountUnmount(system.profile.id);
  expect((await api.connectionsList()).find(e => e.profile.id === system.profile.id)?.connected).toBe(false);
  expect((await api.systemPlacesList()).find(p => p.scheme === "smb")?.mountedAt).toBeNull();
  await api.systemMountConnect(system.profile.id);
  expect((await api.systemPlacesList()).find(p => p.scheme === "smb")?.mountedAt).toBe("/home/demo");
});
