import type { ConnectionEntry, PlacesSnapshot, Settings } from "../ipc/types";
import type { IconName } from "../components/Icon";
import type { TranslationKey } from "../i18n";
import { systemPlaceExists } from "../ipc/systemPlaces";
export interface Place {
  discoveryKey?: string;
  /** Set for a desktop bookmark (Dolphin, GNOME Files) that is not a saved connection yet. */
  systemUrl?: string;
  deviceKey?: string; canRemove?: boolean;
  canSave?: boolean;
  readOnly?: boolean; canMount?: boolean; canUnmount?: boolean; mountKey?: string;
  key: string; name: string; path?: string; icon: IconName; label?: TranslationKey;
  state?: "mounted" | "disconnected" | "connecting" | "unmounted" | "automount";
  reason?: TranslationKey; profileId?: string; color?: string; hint?: number; hidden?: boolean; disabled?: boolean;
  space?: { available: number; total: number } | null; badge?: string;
}
export interface PlaceSection { id: "places" | "remote" | "devices" | "network"; label: TranslationKey; entries: Place[] }
export const inside = (path: string, root: string) => !!root && (path === root || path.startsWith(root === "/" ? "/" : `${root.replace(/\/$/, "")}/`));
export function activePlace(entries: Place[], path: string): string | null {
  return entries.filter(e => e.path && inside(path, e.path)).sort((a, b) => b.path!.length - a.path!.length)[0]?.key ?? null;
}
export function badges(entries: Place[]): Place[] {
  const used = new Set<string>();
  return entries.map(e => {
    const chars = Array.from(e.name.trim().toLocaleUpperCase());
    let badge = chars[0] ?? "?";
    if (used.has(`${e.icon}:${badge}`)) badge = chars.slice(0, 2).join("");
    let n = 2; const base = badge;
    while (used.has(`${e.icon}:${badge}`)) badge = `${base}${n++}`;
    used.add(`${e.icon}:${badge}`);
    return { ...e, badge };
  });
}
export function buildPlaces(snapshot: PlacesSnapshot | null, connections: readonly ConnectionEntry[], settings: Settings, home: string, showHidden = false): PlaceSection[] {
  const places: Place[] = [{ key: "xdg:home", name: home, label: "places.home", path: home, icon: "home" }];
  const icons: Record<string, IconName> = { desktop: "computer", documents: "text", downloads: "downloads", music: "music", pictures: "images", videos: "video" };
  for (const d of snapshot?.userDirs ?? []) if (d.path !== home) places.push({ key: `xdg:${d.kind}`, name: d.kind, label: `sidebar.${d.kind}` as TranslationKey, path: d.path, icon: icons[d.kind]! });
  for (const [i, b] of [...settings.bookmarks].sort((a, b) => a.order - b.order).entries()) places.push({ key: `bookmark:${b.path}`, name: b.name, path: b.path, icon: "pin", color: b.color, hint: i < 9 ? i + 1 : undefined });
  places.push({ key: "trash:home", name: "Trash", label: snapshot?.trash.empty === false ? "sidebar.trashFull" : "sidebar.trash", icon: "trash" });
  const remote: Place[] = connections.map(e => ({ key: `conn:${e.profile.id}`, profileId: e.profile.id, canUnmount: e.connected && e.profile.protocol === "system", name: `${e.profile.name} · ${e.profile.protocol.toUpperCase()}`, path: e.connected ? e.root ?? undefined : undefined, icon: "server", state: e.connected ? "mounted" : "disconnected" }));
  const devices: Place[] = [];
  for (const d of snapshot?.drives ?? []) {
    const automount = !d.mounted && d.fstab?.automount;
    const device = d.deviceActions;
    const mayMount = !!d.fstab?.userMountable || !!device && (!d.fstab || !d.fstab.permission) && !device.encrypted;
    const e: Place = { deviceKey: device?.key, canRemove: device?.removable, disabled: device?.encrypted,
      key: d.fstab || d.source === "fstab" ? `fstab:${d.path}` : device ? `drive:device-${device.key}` : `drive:${d.path}`, name: d.name, path: d.path || undefined, icon: device?.optical ? "optical" : d.kind === "removable" ? "usb" : d.kind === "network" ? "network" : "disk", canSave: ["cifs", "smb3", "nfs", "nfs4", "fuse.sshfs", "davfs", "davfs2"].includes(d.fstype ?? ""), readOnly: d.readOnly, canMount: mayMount && !automount && d.mounted === false, canUnmount: mayMount && d.mounted !== false, state: automount ? "automount" : d.mounted === false ? "unmounted" : "mounted", space: d.space,
      reason: device?.encrypted ? "sidebar.encryptedHelp" : d.mounted === false ? automount ? "sidebar.automountHelp" : mayMount ? "sidebar.unmountedHelp" : "sidebar.adminHelp" : undefined };
    (d.kind === "network" ? remote : devices).push(e);
  }
  for (const m of snapshot?.networkMounts ?? []) {
    if (connections.some(e => (m.mountedAt && e.root === m.mountedAt) || (e.profile.protocol === "system" && e.profile.systemUrl === m.url))) continue;
    const key = `mount:${m.mountedAt ?? m.url}`;
    if (remote.some(e => e.key === key)) continue;
    remote.push({ key, canSave: !!m.mountKey, mountKey: m.mountKey, canUnmount: m.canUnmount, name: `${m.name} · ${m.scheme.toUpperCase()}`, path: m.mountedAt ?? undefined, icon: "network", state: "mounted", disabled: !m.mountedAt, reason: m.mountedAt ? undefined : "sidebar.redactedMount" });
  }
  // Desktop bookmarks come last in the section. One that is already saved as a connection is
  // shown by that connection only (same comparison as the import dialog); mounted ones were
  // removed by the backend. Only name and protocol are displayed, never the URL or account.
  const profiles = connections.map(e => e.profile);
  for (const p of snapshot?.systemPlaces ?? []) {
    const key = `system:${p.url}`;
    if (systemPlaceExists(p, profiles) || remote.some(e => e.key === key)) continue;
    remote.push({ key, systemUrl: p.url, canSave: true, name: `${p.name} · ${p.scheme.toUpperCase()}`, icon: "network", state: "disconnected", reason: "sidebar.systemPlaceHelp" });
  }
  const network: Place[] = settings.sidebarDiscovery ? (snapshot?.discoveredServices ?? []).map(service => ({
    key: `net:${service.key}`, discoveryKey: service.key, name: `${service.name} · ${service.protocol.toUpperCase()} · ${service.host.includes(":") ? `[${service.host}]` : service.host}:${service.port}`,
    icon: "network", reason: "sidebar.discoveredHelp",
  })) : [];
  return ([{ id: "places", label: "sidebar.places", entries: places }, { id: "remote", label: "sidebar.remote", entries: remote }, { id: "devices", label: "places.drives", entries: devices }, ...(network.length ? [{ id: "network", label: "sidebar.network", entries: network }] : [])] as PlaceSection[]).map(section => ({ ...section, entries: badges(section.entries.map(e => ({ ...e, hidden: (settings.sidebarHidden ?? []).includes(e.key) })).filter(e => showHidden || !e.hidden)) }));
}
