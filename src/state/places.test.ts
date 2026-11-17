import { describe, expect, test } from "vitest";
import { activePlace, badges, buildPlaces } from "./places";
import { defaultSettings } from "../app/state";
import { createMockApi } from "../ipc/mock";
import { newSshProfile } from "../components/dialogs/ConnectionProfileForm";
import { effectiveBindings, mapKey } from "./keymap";

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
