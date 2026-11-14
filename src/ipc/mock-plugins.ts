// Plugin-Verwaltung des Mocks (Zustand pro Mock-Instanz): Demo-Plugins in allen Zuständen, Aktivieren
// mit denselben Regeln wie der Kern, Spalten und Vorschau. Das Demo-Dateisystem hängt `mock.ts`
// unter `plugin:<id>!/` ein; die Pfadregeln selbst stehen dort.
import { CoreError } from "./api";
import {
  MAX_PLUGIN_CELL_CHARS,
  MAX_PLUGIN_COLUMN_IDS,
  MAX_PLUGIN_COLUMN_PATHS,
  MAX_PLUGIN_COLUMN_TITLE_CHARS,
  MAX_PLUGIN_DIAGNOSTIC_CHARS,
  MAX_PLUGIN_NAME_CHARS,
  MAX_PLUGIN_PREVIEW_TEXT_CHARS,
} from "./types";
import type {
  Entry,
  PluginColumn,
  PluginColumnValues,
  PluginInfo,
  PluginKind,
  PluginPreview,
  PluginSandboxProbe,
  PluginStatus,
} from "./types";

// ---------------------------------------------------------------- Bereinigung

const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;
const CONTROL_KEEP_LAYOUT = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g;
const MAX_VERSION_CHARS = 32;
const MAX_PROBLEM_CHARS = 200;

/** Kürzt auf `max` Zeichen, ohne ein Ersatzpaar zu zerreißen. */
export function clipText(text: string, max: number): string {
  if (text.length <= max) return text;
  const cut = text.slice(0, max);
  const last = cut.charCodeAt(cut.length - 1);
  return last >= 0xd800 && last <= 0xdbff ? cut.slice(0, -1) : cut;
}

/** Steuerzeichen → U+FFFD, Länge begrenzt; `keepLayout` lässt `\n` und `\t` stehen (Vorschautext). */
export function sanitizePluginText(text: string, max: number, keepLayout = false): string {
  return clipText(String(text).replace(keepLayout ? CONTROL_KEEP_LAYOUT : CONTROL, "�"), max);
}

// ---------------------------------------------------------------- Demo-Plugins

/** Rohdefinition eines Mock-Plugins; Texte werden erst bei der Ausgabe bereinigt. */
export interface MockPluginDef {
  id: string;
  name: string;
  version: string;
  kinds: PluginKind[];
  status: PluginStatus;
  wantsWrite?: boolean;
  writeGranted?: boolean;
  unsandboxed?: boolean;
  /** 64 Hexziffern; Standard: aus der Id abgeleitet. */
  manifestHash?: string;
  /** Standard `true`. */
  sandboxAvailable?: boolean;
  failures?: number;
  disabledReason?: "failures" | "locked" | null;
  problem?: string | null;
  diagnostic?: string;
  columns?: PluginColumn[];
  packerExtensions?: string[];
}

/** Aus der Id abgeleiteter Hash: 64 Hexziffern, stabil, nur für den Entwurf. */
export function demoManifestHash(id: string): string {
  let state = 0x811c9dc5;
  let out = "";
  while (out.length < 64) {
    for (const ch of id) state = Math.imul(state ^ ch.charCodeAt(0), 0x01000193) >>> 0;
    state = Math.imul(state ^ out.length, 0x01000193) >>> 0;
    out += state.toString(16).padStart(8, "0");
  }
  return out.slice(0, 64);
}

export const DEMO_PLUGINS: readonly MockPluginDef[] = [
  {
    id: "demo-fs",
    name: "Demo-Dateisystem",
    version: "1.0.0",
    kinds: ["filesystem", "column", "preview"],
    status: "enabled",
    wantsWrite: true,
    columns: [
      { id: "demo-fs:laenge", title: "Länge" },
      { id: "demo-fs:gross", title: "Gross" },
    ],
  },
  { id: "demo-new", name: "Demo-Packer", version: "0.3.0", kinds: ["packer"], status: "new", packerExtensions: ["dfz"] },
  { id: "demo-off", name: "Demo-Spalten", version: "1.2.0", kinds: ["column"], status: "disabled" },
  {
    id: "demo-locked",
    name: "Demo-Geändert",
    version: "2.0.0",
    kinds: ["filesystem"],
    status: "locked",
    problem: "Das Plugin wurde verändert",
    disabledReason: "locked",
  },
  { id: "demo-nosandbox", name: "Demo-Ohne-Sandbox", version: "1.0.0", kinds: ["preview"], status: "sandboxUnavailable", sandboxAvailable: false },
  {
    id: "demo-failed",
    name: "Demo-Ausgefallen",
    version: "0.9.0",
    kinds: ["preview"],
    status: "failed",
    failures: 3,
    disabledReason: "failures",
    problem: "Das Plugin ist mehrfach ausgefallen",
    diagnostic: "Zeile 1: Verbindung zum Plugin-Prozess verloren",
  },
];

const PROBE_OK: PluginSandboxProbe = { available: true, landlockAbi: 4, landlockNet: true, landlockScoped: true };
const PROBE_NONE: PluginSandboxProbe = { available: false, landlockAbi: 0, landlockNet: false, landlockScoped: false };

// ---------------------------------------------------------------- Demo-Dateisystem

/** Knoten des Demo-Dateisystems; passt strukturell zum Knoten in `mock.ts`. */
export interface PluginNode {
  entry: Entry;
  children?: Map<string, PluginNode>;
  version?: number;
}

function demoFsTree(now: number): PluginNode {
  const at = now - 86_400_000 * 2;
  const entry = (name: string, kind: "file" | "dir", size: number): Entry => ({
    name,
    kind,
    size,
    modified: at,
    mode: kind === "dir" ? 0o755 : 0o644,
    hidden: false,
    linkTarget: null,
    brokenLink: false,
    error: null,
  });
  const dir = (name: string, children: PluginNode[] = []): PluginNode => ({
    entry: entry(name, "dir", 0),
    children: new Map(children.map((child) => [child.entry.name, child])),
  });
  const file = (name: string, size: number): PluginNode => ({ entry: entry(name, "file", size) });
  return dir("", [
    file("liesmich.txt", 256),
    file("bild.png", 70),
    dir("dokumente", [file("notizen.txt", 640), file("liste.csv", 1_024)]),
    dir("leer"),
  ]);
}

/** Kleinstes gültiges PNG (1x1, ein Pixel), Base64. */
export const TINY_PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAEBgIApD5fRAAAAABJRU5ErkJggg==";

// ---------------------------------------------------------------- Verwaltung

/** Zugriff auf das (restliche) Mock-Dateisystem für Spalten und Vorschau. */
export interface PluginHost {
  /** `null`, wenn es den Pfad nicht gibt; wirft wie die Pfadauflösung (`invalidPath`, `disconnected`). */
  stat(path: string): { name: string; dir: boolean; size: number } | null;
  /** Liest höchstens `len` Bytes vom Anfang einer Datei. */
  read(path: string, len: number): Uint8Array;
}

export type PluginWriteAccess = "inactive" | "readOnly" | "writable";

interface PluginRecord {
  def: MockPluginDef;
  status: PluginStatus;
  writeGranted: boolean;
  unsandboxed: boolean;
  failures: number;
  disabledReason: "failures" | "locked" | null;
  problem: string | null;
  tree: PluginNode | null;
}

const fail = (message: string, kind: string): CoreError => new CoreError(message, kind);
const notFound = (): CoreError => fail("Das Plugin gibt es nicht", "notFound");

export interface MockPlugins {
  list(): PluginInfo[];
  activate(id: string, manifestHash: string, grantWrite: boolean, unsandboxed: boolean, confirmUnsandboxed: boolean): PluginInfo;
  deactivate(id: string): PluginInfo;
  trustReset(id: string): void;
  diagnostics(id: string): string;
  columns(paths: string[], columnIds: string[], host: PluginHost): PluginColumnValues[];
  preview(path: string, pluginId: string | undefined, host: PluginHost): PluginPreview;
  /** Wurzel des Demo-Dateisystems, nur wenn das Plugin aktiv ist und die Art `filesystem` hat. */
  tree(id: string): PluginNode | null;
  writeAccess(id: string): PluginWriteAccess;
}

export function createMockPlugins(now: () => number, extra: readonly MockPluginDef[] = []): MockPlugins {
  const records = new Map<string, PluginRecord>();
  for (const def of [...DEMO_PLUGINS, ...extra]) {
    records.set(def.id, {
      def,
      status: def.status,
      writeGranted: def.wantsWrite === true && def.writeGranted === true,
      unsandboxed: def.unsandboxed === true,
      failures: def.failures ?? 0,
      disabledReason: def.disabledReason ?? null,
      problem: def.problem ?? null,
      tree: null,
    });
  }

  const need = (id: string): PluginRecord => {
    const found = typeof id === "string" ? records.get(id) : undefined;
    if (!found) throw notFound();
    return found;
  };
  const hashOf = (rec: PluginRecord): string => rec.def.manifestHash ?? demoManifestHash(rec.def.id);
  const isActive = (rec: PluginRecord): boolean => rec.status === "enabled";

  const info = (rec: PluginRecord): PluginInfo => {
    const { def } = rec;
    const enabled = isActive(rec);
    return {
      id: def.id,
      name: sanitizePluginText(def.name, MAX_PLUGIN_NAME_CHARS),
      version: sanitizePluginText(def.version, MAX_VERSION_CHARS),
      kinds: [...def.kinds],
      status: rec.status,
      wantsWrite: def.wantsWrite === true,
      writeGranted: rec.writeGranted,
      unsandboxed: rec.unsandboxed,
      enabled,
      manifestHash: hashOf(rec),
      sandbox: { ...(def.sandboxAvailable === false ? PROBE_NONE : PROBE_OK) },
      failures: rec.failures,
      disabledReason: rec.disabledReason,
      problem: rec.problem === null ? null : sanitizePluginText(rec.problem, MAX_PROBLEM_CHARS),
      diagnostic: sanitizePluginText(def.diagnostic ?? "", MAX_PLUGIN_DIAGNOSTIC_CHARS),
      columns: (def.columns ?? []).map((column) => ({
        id: column.id,
        title: sanitizePluginText(column.title, MAX_PLUGIN_COLUMN_TITLE_CHARS),
      })),
      packerExtensions: (def.packerExtensions ?? []).map((ext) => ext.toLowerCase().replace(/^\./, "")),
      filesystemRoot: enabled && def.kinds.includes("filesystem") ? `plugin:${def.id}!/` : null,
    };
  };

  const activeColumn = (columnId: string): { rec: PluginRecord; local: string } => {
    for (const rec of records.values()) {
      if (!isActive(rec) || !rec.def.kinds.includes("column")) continue;
      if ((rec.def.columns ?? []).some((column) => column.id === columnId)) {
        return { rec, local: columnId.slice(rec.def.id.length + 1) };
      }
    }
    throw fail("Die Spalte gibt es nicht oder ihr Plugin ist nicht aktiv", "notFound");
  };

  const cell = (local: string, name: string): string | null => {
    if (local === "laenge") return String(Array.from(name).length);
    if (local === "gross") return sanitizePluginText(name.toUpperCase(), MAX_PLUGIN_CELL_CHARS);
    return null;
  };

  const decoder = new TextDecoder("utf-8", { fatal: false });
  const extensionOf = (name: string): string => {
    const dot = name.lastIndexOf(".");
    return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  };

  return {
    list: () => [...records.values()].map(info),

    activate(id, manifestHash, grantWrite, unsandboxed, confirmUnsandboxed) {
      const rec = need(id);
      if (manifestHash !== hashOf(rec) || rec.status === "locked") {
        throw fail("Das Plugin ist gesperrt oder wurde verändert", "pluginLocked");
      }
      if (unsandboxed === true && confirmUnsandboxed !== true) {
        throw fail("Ungültige Eingabe: „Ohne Sandbox“ wurde nicht bestätigt", "invalidInput");
      }
      if (rec.def.sandboxAvailable === false && unsandboxed !== true) {
        throw fail("Auf diesem System steht keine Sandbox bereit", "pluginSandboxUnavailable");
      }
      rec.status = "enabled";
      rec.writeGranted = rec.def.wantsWrite === true && grantWrite === true;
      rec.unsandboxed = unsandboxed === true;
      rec.failures = 0;
      rec.disabledReason = null;
      rec.problem = null;
      return info(rec);
    },

    deactivate(id) {
      const rec = need(id);
      if (rec.status === "enabled") rec.status = "disabled";
      return info(rec);
    },

    trustReset(id) {
      const rec = need(id);
      rec.status = "new";
      rec.writeGranted = false;
      rec.unsandboxed = false;
      rec.failures = 0;
      rec.disabledReason = null;
      rec.problem = null;
    },

    diagnostics: (id) => info(need(id)).diagnostic,

    columns(paths, columnIds, host) {
      if (!Array.isArray(paths) || !Array.isArray(columnIds) || paths.length > MAX_PLUGIN_COLUMN_PATHS || columnIds.length > MAX_PLUGIN_COLUMN_IDS) {
        throw fail(
          `Ungültige Eingabe: höchstens ${MAX_PLUGIN_COLUMN_PATHS} Pfade und ${MAX_PLUGIN_COLUMN_IDS} Spalten je Aufruf`,
          "invalidInput",
        );
      }
      const wanted = columnIds.map((columnId) => ({ columnId, ...activeColumn(columnId) }));
      const stats = paths.map((path) => host.stat(path));
      return wanted.map(({ columnId, local }) => ({
        columnId,
        values: stats.map((stat) => (stat === null || stat.dir ? null : cell(local, stat.name))),
      }));
    },

    preview(path, pluginId, host) {
      let rec: PluginRecord | undefined;
      if (pluginId !== undefined && pluginId !== null) {
        rec = need(pluginId);
        if (!isActive(rec)) throw fail("Das Plugin ist nicht aktiv", "pluginDisabled");
      } else {
        rec = [...records.values()].find((candidate) => isActive(candidate) && candidate.def.kinds.includes("preview"));
      }
      if (!rec || !rec.def.kinds.includes("preview")) {
        throw fail("Dafür gibt es keine Vorschau", "unsupported");
      }
      const stat = host.stat(path);
      if (stat === null) throw fail("Nicht gefunden", "notFound");
      if (stat.dir) throw fail("Für Ordner gibt es keine Vorschau", "unsupported");
      const ext = extensionOf(stat.name);
      if (ext === "png") return { kind: "image", mime: "image/png", data: TINY_PNG_BASE64 };
      if (ext === "txt") {
        const bytes = host.read(path, MAX_PLUGIN_PREVIEW_TEXT_CHARS * 4);
        const text = sanitizePluginText(decoder.decode(bytes), MAX_PLUGIN_PREVIEW_TEXT_CHARS, true);
        return { kind: "text", text };
      }
      throw fail("Dafür gibt es keine Vorschau", "unsupported");
    },

    tree(id) {
      const rec = records.get(id);
      if (!rec || !isActive(rec) || !rec.def.kinds.includes("filesystem")) return null;
      rec.tree ??= demoFsTree(now());
      return rec.tree;
    },

    writeAccess(id) {
      const rec = records.get(id);
      if (!rec || !isActive(rec) || !rec.def.kinds.includes("filesystem")) return "inactive";
      return rec.writeGranted && rec.def.wantsWrite === true ? "writable" : "readOnly";
    },
  };
}
