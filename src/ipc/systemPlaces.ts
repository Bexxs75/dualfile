import type { ConnectionProfile, SystemPlace } from "./types";

export function systemPlaceProfile(place: SystemPlace): ConnectionProfile {
  const protocol = ["sftp", "fish", "ssh"].includes(place.scheme) ? "sftp" : place.scheme === "ftp" ? "ftp" : place.scheme === "ftps" ? "ftps" : place.scheme === "smb" ? "smb" : "system";
  return {
    id: "", name: place.name, protocol, transport: protocol === "sftp" ? "openssh" : "builtin",
    host: place.host, port: place.port ?? (protocol === "sftp" || protocol === "system" ? 0 : protocol === "smb" ? 445 : 21),
    user: place.user ?? (protocol === "ftp" || protocol === "ftps" ? "anonymous" : ""),
    share: protocol === "smb" ? place.path.split("/").filter(Boolean)[0] ?? "" : "",
    startFolder: protocol === "smb" ? "/" + place.path.split("/").filter(Boolean).slice(1).join("/") : place.path, tls: protocol === "ftps" ? "explicit" : "none",
    allowPlaintext: false, auth: { kind: "password" }, saveSecret: false,
    systemUrl: protocol === "system" ? place.url : null,
  };
}
export function systemPlaceExists(place: SystemPlace, profiles: ConnectionProfile[]): boolean {
  const target = systemPlaceProfile(place);
  const port = (p: ConnectionProfile) => p.port === 0 && p.protocol === "sftp" ? 22 : p.port;
  return profiles.some(p => p.protocol === target.protocol && p.host.toLowerCase() === target.host.toLowerCase()
    && port(p) === port(target) && p.user === target.user && (p.startFolder ?? "/") === target.startFolder
    && (p.share ?? "") === (target.share ?? "") && (p.systemUrl ?? null) === target.systemUrl);
}
export const mockSystemPlaces: SystemPlace[] = [
  { sources: ["dolphin", "gnome"], name: "Home server", scheme: "sftp", host: "nas", port: null, user: null, path: "/", url: "sftp://nas/", mountedAt: null },
  { sources: ["mounted"], name: "Team share", scheme: "smb", host: "nas", port: null, user: null, path: "/team", url: "smb://nas/team", mountedAt: "/home/demo" },
];
