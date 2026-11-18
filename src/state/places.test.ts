import { describe, expect, test } from "vitest";
import { activePlace, badges, buildPlaces } from "./places";
import { defaultSettings } from "../app/state";
import { createMockApi } from "../ipc/mock";
import { newSshProfile } from "../components/dialogs/ConnectionProfileForm";
import { effectiveBindings, mapKey } from "./keymap";
import { systemPlaceProfile } from "../ipc/systemPlaces";

describe("read-only places", () => {
  test("sections, XDG home exclusion, bookmark order and hidden filtering", async () => {
    const snapshot = await createMockApi().placesSnapshot();
    snapshot.userDirs.push({ kind: "music", path: snapshot.home });
    const settings = { ...defaultSettings, bookmarks: [{ name: "B", path: "/b", order: 2, color: "#000000" }, { name: "A", path: "/a", order: 1, color: "#ffffff" }], sidebarHidden: ["drive:/"] };
    const sections = buildPlaces(snapshot, [], settings, snapshot.home);
    expect(sections.map(s => s.id)).toEqual(["places", "remote", "devices"]);
    expect(sections[0]!.entries.filter(e => e.key.startsWith("bookmark:")).map(e => [e.name, e.hint])).toEqual([["A", 1], ["B", 2]]);
    expect(sections[0]!.entries.some(e => e.key === "xdg:music")).toBe(false);
    expect(sections[2]!.entries.some(e => e.path === "/")).toBe(false);
    expect(buildPlaces(snapshot, [], settings, snapshot.home, true)[2]!.entries.find(e => e.path === "/")?.hidden).toBe(true);
    expect(sections[1]!.entries.find(e => e.key === "fstab:/mnt/nas")).toMatchObject({ state: "unmounted", reason: "sidebar.unmountedHelp" });
  });
  test("longest path prefix and path boundaries", () => {
    const entries = [{ key: "root", path: "/", name: "/", icon: "disk" as const }, { key: "usb", path: "/run/media/u/X", name: "X", icon: "usb" as const }];
    expect(activePlace(entries, "/run/media/u/X/sub")).toBe("usb");
    expect(activePlace(entries, "/run/media/u/XX")).toBe("root");
  });
  test("badges disambiguate repeated initials and names", () => {
    const result = badges(["Sicherung", "Sammlung", "Sammlung"].map((name, i) => ({ key: `${i}`, name, icon: "disk" })));
    expect(new Set(result.map(e => e.badge)).size).toBe(3);
  });
  test("existing F9 and Shift+F9 bindings win; new commands remain unbound", () => {
    const bindings = { copy: "F9", rename: "Shift+F9" };
    expect(effectiveBindings(bindings)).toMatchObject({ toggleSidebar: "", toggleSidebarIcons: "" });
    expect(mapKey({ key: "F9" }, "list", bindings)).toBe("copy");
    expect(mapKey({ key: "F9", shiftKey: true }, "list", bindings)).toBe("rename");
    expect(mapKey({ key: "F9" })).toBe("toggleSidebar");
  });
});

test("redacted mounted locations remain visible with a disabled explanation", async () => {
  const snapshot = await createMockApi().placesSnapshot();
  snapshot.networkMounts = [{ sources: ["mounted"], name: "host/share", scheme: "sftp", host: "host", port: null, user: null, path: "/share", url: "sftp://host/share", mountedAt: null }];
  const remote = buildPlaces(snapshot, [], defaultSettings, snapshot.home)[1]!.entries;
  expect(remote.find(e => e.key === "mount:sftp://host/share")).toMatchObject({ name: "host/share · SFTP", state: "mounted", disabled: true, reason: "sidebar.redactedMount" });
});


test("disconnected saved connections do not hide redacted mounts through null roots", async () => {
  const api = createMockApi();
  await api.connectionSave(newSshProfile("offline-host"));
  const connections = await api.connectionsList();
  expect(connections.some(e => !e.connected && e.root === null)).toBe(true);
  const snapshot = await api.placesSnapshot();
  snapshot.networkMounts = [{ sources: ["mounted"], name: "host/share", scheme: "sftp", host: "host", port: null, user: null, path: "/share", url: "sftp://host/share", mountedAt: null }];
  const entries = buildPlaces(snapshot, connections, defaultSettings, snapshot.home)[1]!.entries;
  expect(entries.find(e => e.key === "mount:sftp://host/share")).toMatchObject({ disabled: true, reason: "sidebar.redactedMount" });
  expect(entries.some(e => e.state === "disconnected")).toBe(true);
});

test("device rows keep stable keys, support polkit fstab and exclude encrypted mounts", async () => {
  const snapshot = await createMockApi().placesSnapshot();
  const info = { key: "opaque", removable: true, encrypted: false, optical: false };
  snapshot.drives.push({ name: "USB", path: "", device: "/dev/sdb1", mounted: false, kind: "removable", source: "device", deviceActions: info });
  snapshot.drives.push({ name: "Local fstab", path: "/data", mounted: false, fstab: { userMountable: false, noauto: true, automount: false }, deviceActions: { ...info, key: "fixed", removable: false } });
  snapshot.drives.push({ name: "Locked", path: "", mounted: false, deviceActions: { ...info, key: "luks", encrypted: true } });
  const rows = buildPlaces(snapshot, [], defaultSettings, snapshot.home).flatMap(s => s.entries);
  expect(rows.find(r => r.name === "USB")).toMatchObject({ key: "drive:device-opaque", canMount: true, canRemove: true, path: undefined });
  expect(rows.find(r => r.name === "Local fstab")).toMatchObject({ key: "fstab:/data", canMount: true, canRemove: false });
  expect(rows.find(r => r.name === "Locked")).toMatchObject({ disabled: true, canMount: false, reason: "sidebar.encryptedHelp" });
  const usb = snapshot.drives.find(d => d.device === "/dev/sdb1")!;
  usb.path = "/media/USB"; usb.mounted = true;
  expect(buildPlaces(snapshot, [], defaultSettings, snapshot.home)[2]!.entries.find(r => r.name === "USB")).toMatchObject({ key: "drive:device-opaque", canUnmount: true });
});

test("desktop bookmarks appear once as disconnected system places and never show URL or account", async () => {
  const api = createMockApi();
  const snapshot = await api.placesSnapshot();
  const bookmark = (url: string, name: string, scheme: string, host: string, user: string | null = null, path = "/") => ({ sources: ["dolphin"], name, scheme, host, port: null, user, path, url, mountedAt: null });
  snapshot.systemPlaces = [bookmark("sftp://nas/", "Home server", "sftp", "nas"), bookmark("smb://alice@box/team", "Team", "smb", "box", "alice", "/team")];
  const remote = (connections: Parameters<typeof buildPlaces>[1], settings = defaultSettings, showHidden = false) => buildPlaces(snapshot, connections, settings, snapshot.home, showHidden)[1]!.entries;
  const entries = remote([]);
  expect(entries.find(e => e.key === "system:sftp://nas/")).toMatchObject({ name: "Home server · SFTP", state: "disconnected", canSave: true, systemUrl: "sftp://nas/", reason: "sidebar.systemPlaceHelp" });
  const team = entries.find(e => e.key === "system:smb://alice@box/team")!;
  expect(team.path).toBeUndefined();
  expect(`${team.name} ${team.path}`).not.toMatch(/alice|smb:\/\//);
  // Saved connections are listed first; a bookmark that was saved is not repeated below them.
  await api.connectionSave(systemPlaceProfile(snapshot.systemPlaces[0]!));
  const saved = remote(await api.connectionsList());
  expect(saved.some(e => e.key === "system:sftp://nas/")).toBe(false);
  expect(saved.filter(e => e.name.startsWith("Home server"))).toHaveLength(1);
  expect(saved.some(e => e.key === "system:smb://alice@box/team")).toBe(true);
  // "Hide entry" uses the ordinary hidden list, so the choice is stored with the settings.
  const hidden = { ...defaultSettings, sidebarHidden: ["system:smb://alice@box/team"] };
  expect(remote([], hidden).some(e => e.key === "system:smb://alice@box/team")).toBe(false);
  expect(remote([], hidden, true).find(e => e.key === "system:smb://alice@box/team")?.hidden).toBe(true);
});
