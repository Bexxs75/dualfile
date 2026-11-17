import type { ConnectionProfile } from "../ipc/types";

// Kleine Pfadhilfen für die Oberflächenlogik. Lokale Pfade sind POSIX-Pfade; Pfade in Archiven haben das
// Format `archive:<Archivdatei, % und ! kodiert>!<innen>` (Spiegel von `format_ipc_path`/`parse_ipc_path` im Kern).
// Die Archivdatei kann selbst ein Archivpfad sein (verschachtelte Archive).

export interface RemoteConnection {
  profile: ConnectionProfile;
  connected: boolean;
  endpoint: string;
}
export type RemoteConnections = Readonly<Record<string, RemoteConnection>>;

export function parseRemotePath(path: string): { protocol: string; id: string; inner: string; root: string } | null {
  const match = /^(sftp|ftp|ftps|smb):([^!]+)!(\/.*)$/.exec(path);
  return match ? { protocol: match[1]!, id: match[2]!, inner: match[3]!, root: `${match[1]}:${match[2]}!` } : null;
}

/** Ein gemerkter Remote-Pfad kann auf eine alte Verbindungs-ID zeigen (jede Verbindung bekommt eine neue ID).
 * Liefert den Pfad mit der aktuellen ID desselben Profils (bevorzugt verbunden), sonst den Pfad unverändert. */
export function currentRemotePath(path: string, connections: RemoteConnections): string {
  const remote = parseRemotePath(path);
  const profileId = remote ? connections[remote.id]?.profile.id : undefined;
  if (!remote || profileId === undefined) return path;
  const same = Object.entries(connections).filter(([, c]) => c.profile.id === profileId);
  const best = same.find(([, c]) => c.connected) ?? same[same.length - 1];
  return best ? `${remote.protocol}:${best[0]}!${remote.inner}` : path;
}

function remoteLabel(remote: NonNullable<ReturnType<typeof parseRemotePath>>, connections: RemoteConnections): string {
  const profile = connections[remote.id]?.profile;
  if (!profile) return remote.protocol.toUpperCase();
  return profile.user ? `${profile.user}@${profile.host}` : profile.protocol === "smb" ? profile.host : profile.name;
}

const ARCHIVE_PREFIX = "archive:";

/** Endungen, bei denen Enter/Doppelklick eine Datei als Archiv öffnet (längere zuerst nicht nötig: Endvergleich). */
const ARCHIVE_SUFFIXES = [".zip", ".7z", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".rar"] as const;

export const isArchivePath = (path: string): boolean => path.startsWith(ARCHIVE_PREFIX);

/** Hat der Dateiname eine Archivendung? (Groß-/Kleinschreibung egal.) */
export function isArchiveName(name: string): boolean {
  const lower = name.toLowerCase();
  return ARCHIVE_SUFFIXES.some((suffix) => lower.endsWith(suffix) && lower.length > suffix.length);
}

const encodeOuter = (outer: string): string => outer.replaceAll("%", "%25").replaceAll("!", "%21");
const decodeOuter = (encoded: string): string => encoded.replaceAll("%21", "!").replaceAll("%25", "%");

/** Baut den serialisierten Archivpfad. `inner` beginnt mit "/". */
export function formatArchivePath(outer: string, inner: string): string {
  return `${ARCHIVE_PREFIX}${encodeOuter(outer)}!${inner.startsWith("/") ? inner : `/${inner}`}`;
}

/** Zerlegt einen Archivpfad; null für lokale oder fehlerhafte Pfade (andere `%`-Folgen, kein Innenpfad). */
export function parseArchivePath(path: string): { outer: string; inner: string } | null {
  if (!isArchivePath(path)) return null;
  const rest = path.slice(ARCHIVE_PREFIX.length);
  const cut = rest.indexOf("!");
  if (cut < 0) return null;
  const encoded = rest.slice(0, cut);
  const inner = rest.slice(cut + 1);
  if (/%(?!25|21)/.test(encoded) || !inner.startsWith("/")) return null;
  return { outer: decodeOuter(encoded), inner };
}

/** Wurzel des Archivs, dessen Datei `filePath` ist (lokal oder selbst in einem Archiv). */
export const archiveRootOf = (filePath: string): string => formatArchivePath(filePath, "/");

/** Ordner der äußersten Archivdatei eines Archivpfads (der lokale Ordner, in dem sie liegt); sonst null. */
export function archiveHostFolder(path: string): string | null {
  let parsed = parseArchivePath(path);
  if (!parsed) return null;
  while (isArchivePath(parsed.outer)) {
    const next = parseArchivePath(parsed.outer);
    if (!next) return null;
    parsed = next;
  }
  return parentPath(parsed.outer);
}

function normalizeLocal(path: string): string {
  const collapsed = path.replace(/\/{2,}/g, "/");
  return collapsed.length > 1 ? collapsed.replace(/\/+$/, "") || "/" : collapsed;
}

/** Entfernt abschließende Schrägstriche und doppelte Trenner; "/" bleibt "/". Archivpfade: Archivdatei und Innenpfad je für sich. */
/** Pseudo path of the trash view tab; never a real folder, never sent to the backend. */
export const TRASH_PATH = "trash:";
export const isTrashPath = (path: string): boolean => path === TRASH_PATH;

export function normalizePath(path: string): string {
  if (isTrashPath(path)) return path;
  const remote = parseRemotePath(path);
  if (remote) return `${remote.root}${normalizeLocal(remote.inner)}`;
  const parsed = parseArchivePath(path);
  if (parsed) return formatArchivePath(normalizePath(parsed.outer), normalizeLocal(parsed.inner));
  return normalizeLocal(path);
}

/**
 * Eine Ebene hoch; die Wurzel hat kein Elternverzeichnis und bleibt "/". Das Elternverzeichnis der
 * Archivwurzel ist der Ordner der Archivdatei (bei verschachtelten Archiven der Ordner im äußeren Archiv).
 */
export function parentPath(path: string): string {
  const remote = parseRemotePath(path);
  if (remote) return `${remote.root}${parentPath(remote.inner)}`;
  const parsed = parseArchivePath(path);
  if (parsed) {
    const inner = normalizeLocal(parsed.inner);
    if (inner === "/") return parentPath(parsed.outer);
    return formatArchivePath(normalizePath(parsed.outer), parentPath(inner));
  }
  const norm = normalizeLocal(path);
  const cut = norm.lastIndexOf("/");
  return cut <= 0 ? "/" : norm.slice(0, cut);
}

/** Letzter Pfadteil; für die Wurzel "/", für eine Archivwurzel der Name der Archivdatei. */
export function baseName(path: string): string {
  const remote = parseRemotePath(path);
  if (remote) return baseName(remote.inner);
  const parsed = parseArchivePath(path);
  if (parsed) {
    const inner = normalizeLocal(parsed.inner);
    return inner === "/" ? baseName(parsed.outer) : baseName(inner);
  }
  const norm = normalizeLocal(path);
  return norm === "/" ? "/" : norm.slice(norm.lastIndexOf("/") + 1);
}

export function joinPath(dir: string, name: string): string {
  const parsed = parseArchivePath(dir);
  if (parsed) return formatArchivePath(normalizePath(parsed.outer), joinPath(parsed.inner, name));
  const norm = normalizeLocal(dir);
  return norm === "/" ? `/${name}` : `${norm}/${name}`;
}

export interface Crumb {
  label: string;
  /** Serialisierter Pfad, zu dem ein Klick führt. */
  path: string;
}

/** Brotkrumen eines Pfads: lokal "/" und die Ordner; im Archiv zusätzlich die Archivdatei (= Wurzel) und die Innenpfade. */
export function crumbsOf(path: string, connections: RemoteConnections = {}): Crumb[] {
  const remote = parseRemotePath(path);
  if (remote) {
    const parts = normalizeLocal(remote.inner).split("/").filter(Boolean);
    return [{ label: remoteLabel(remote, connections), path: `${remote.root}/` },
      ...(connections[remote.id]?.profile.protocol === "smb" && connections[remote.id]?.profile.share ? [{ label: connections[remote.id]!.profile.share!, path: `${remote.root}/` }] : []),
      ...parts.map((label, i) => ({ label, path: `${remote.root}/${parts.slice(0, i + 1).join("/")}` }))];
  }
  const parsed = parseArchivePath(path);
  if (!parsed) {
    const parts = normalizeLocal(path).split("/").filter(Boolean);
    return [
      { label: "/", path: "/" },
      ...parts.map((part, index) => ({ label: part, path: `/${parts.slice(0, index + 1).join("/")}` })),
    ];
  }
  const outerCrumbs = crumbsOf(normalizePath(parsed.outer), connections);
  const last = outerCrumbs[outerCrumbs.length - 1];
  const root = archiveRootOf(normalizePath(parsed.outer));
  const head = last ? [...outerCrumbs.slice(0, -1), { label: last.label, path: root }] : [];
  const outer = normalizePath(parsed.outer);
  const parts = normalizeLocal(parsed.inner).split("/").filter(Boolean);
  return [...head, ...parts.map((part, index) => ({ label: part, path: formatArchivePath(outer, `/${parts.slice(0, index + 1).join("/")}`) }))];
}

/** Lesbarer Pfad für Anzeige: `/ordner/archiv.zip/innen/pfad` (nie das kodierte Rohformat). */
export function displayPath(path: string, connections: RemoteConnections = {}): string {
  const remote = parseRemotePath(path);
  if (remote) return `${remoteLabel(remote, connections)}${normalizeLocal(remote.inner)}`;
  const parsed = parseArchivePath(path);
  if (!parsed) return path;
  const inner = normalizeLocal(parsed.inner);
  const outer = displayPath(parsed.outer, connections);
  return inner === "/" ? outer : `${outer}${inner}`;
}

/** Relative conflict path only when the confirmed transfer root is known. */
export function conflictDisplayPath(path: string, destination?: string): string {
  if (!destination) return path;
  const prefix = `${destination.replace(/\/+$/, "")}/`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
}
