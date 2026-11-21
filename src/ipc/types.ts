// Vertrag zwischen Oberfläche und Rust-Kern (`dualfile-core`). Spiegelt die Rust-Typen;
// Teil 1c verdrahtet ihn mit den Tauri-Befehlen. Die Oberfläche greift nur hierüber auf Dateien zu.

export type EntryKind = "file" | "dir" | "symlink";

/** Ort und Größe der Logdatei; `path` mit `~` statt Home-Ordner. */
export interface LogInfo {
  path: string;
  bytes: number;
}

export type LogLevel = "error" | "warn" | "info";

export interface Entry {
  name: string;
  kind: EntryKind;
  size: number;
  /** Millisekunden seit 1970, null wenn unbekannt. */
  modified: number | null;
  mode: number | null;
  uid?: number | null;
  gid?: number | null;
  ownerName?: string | null;
  groupName?: string | null;
  permissions?: string | null;
  hidden: boolean;
  linkTarget: string | null;
  brokenLink: boolean;
  /** Fehler bei genau diesem Eintrag; die übrigen Einträge sind trotzdem da. */
  error: string | null;
}

export interface Listing {
  /** Absoluter, normalisierter Pfad. */
  path: string;
  entries: Entry[];
}

/** Art einer Datei für den Lister (Erkennung per Signatur, nicht nur Endung). */
export type FileKind = "text" | "binary" | "image" | "audio" | "video";

export interface FileInfo {
  /** Größe in Bytes (folgt Links; nur reguläre Dateien). */
  size: number;
  kind: FileKind;
  /** Millisekunden seit 1970, null wenn unbekannt. */
  modified: number | null;
  mimeGuess: string;
}

/** EXIF-/ID3-Metadaten einer Datei für die Umbenennen-Platzhalter; leere Felder bei Fehlen. */
export interface MediaTags {
  /** EXIF-Aufnahmezeitpunkt als "YYYY:MM:DD HH:MM:SS", leer wenn nicht vorhanden. */
  taken: string;
  camera: string;
  artist: string;
  title: string;
  album: string;
  /** ID3-Titelnummer, 0 wenn nicht vorhanden. */
  track: number;
}

export type SearchDirection = "forward" | "backward";

/**
 * Ergebnis einer Suche. `continue`: Suchgrenze je Aufruf erreicht, ohne Treffer; mit `next` als
 * `start` (gleiche Richtung) weitersuchen.
 */
export type SearchResult =
  | { kind: "found"; offset: number }
  | { kind: "notFound" }
  | { kind: "continue"; next: number };

/** Obergrenzen, die der Kern erzwingt (Werte siehe `lister.rs`). */
export const MAX_BLOCK_BYTES = 1024 * 1024;
export const MAX_NEEDLE_BYTES = 4096;

/** Archivformate, die `packArchive` schreiben kann (RAR gibt es nur zum Lesen). */
export type PackFormat = "zip" | "tarGz" | "tarBz2" | "tarXz" | "sevenZ";

export const PACK_FORMATS: readonly PackFormat[] = ["zip", "tarGz", "tarBz2", "tarXz", "sevenZ"];

export type Resolution = "overwrite" | "skip" | "keepBoth" | "cancel";

export interface ConflictInfo {
  jobId: number;
  sourcePath: string;
  destinationPath: string;
  source: Entry;
  destination: Entry;
}

export interface ConflictAnswer {
  resolution: Resolution;
  /** „für alle“ */
  applyToAll: boolean;
}

export type JobState = "queued" | "running" | "paused" | "done" | "failed" | "cancelled";

/** Sprachneutraler Auftragstitel; die Oberfläche bildet daraus „Kopieren: x (+2)“ in ihrer Sprache. */
export interface JobTitle {
  kind: string;
  first: string;
  more: number;
}

export interface JobStatus {
  verified?: boolean;
  /** Fehlt bei älteren Kernen; dann gilt `name` (deutscher Rückfalltext). */
  title?: JobTitle;
  id: number;
  name: string;
  state: JobState;
  filesDone: number;
  bytesDone: number;
  bytesTotal: number;
  bytesPerSecond: number;
  etaSeconds: number | null;
  queuePosition: number | null;
  lane: string | null;
  failed: { path: string; message: string; kind?: string }[];
  skipped: string[];
  warnings: { path: string; message: string }[];
}

export interface DeletePlan {
  token: string;
  files: number;
  dirs: number;
  links: number;
  bytes: number;
  firstNames: string[];
  incomplete: boolean;
}

/** Ein Eintrag für `renameMany`: Namen innerhalb des Ordners, keine Pfade. */
export interface RenameItem {
  oldName: string;
  newName: string;
}

/** Ergebnis je Name von `regexReplace`; Reihenfolge wie die Eingabe. */
export type RegexResult = { kind: "ok"; name: string } | { kind: "error"; message: string };

/** Grenzen von `regexReplace` (Kern: `regex_tools.rs`); der Mock hält dieselben ein. */
export const MAX_REGEX_PATTERN_CHARS = 1000;
export const MAX_REGEX_NAME_BYTES = 255;
export const MAX_REGEX_NAMES = 10_000;
/** Längere Ergebnisse einer Ersetzung werden als Fehler dieses Namens gemeldet. */
export const MAX_REGEX_RESULT_BYTES = 4096;

// ---------------------------------------------------------------- Vergleichen und Synchronisieren

/** Zustand eines Paares beim Ordnervergleich (`CompareState` im Kern). */
export type CompareState =
  | "onlyLeft"
  | "onlyRight"
  | "same"
  | "leftNewer"
  | "rightNewer"
  /** Gleiche Zeit (2 s Toleranz), aber andere Größe oder anderer Inhalt. */
  | "different"
  /** Datei, Ordner und Verknüpfung passen nicht zusammen. */
  | "typeMismatch"
  /** Nicht untersucht (nicht lesbar, Grenze erreicht): nie als leer oder „fehlt“ gewertet. */
  | "unknown";

export const COMPARE_STATES: readonly CompareState[] = [
  "onlyLeft",
  "onlyRight",
  "same",
  "leftNewer",
  "rightNewer",
  "different",
  "typeMismatch",
  "unknown",
];

/** `sizeTime`: Größe und Zeit (Standard); `content`: Byte für Byte. */
export type CompareMode = "sizeTime" | "content";

export interface CompareOptions {
  mode: CompareMode;
  recursive: boolean;
}

export interface CompareSide {
  kind: EntryKind;
  size: number;
  /** Millisekunden seit 1970, null wenn unbekannt. */
  modified: number | null;
  linkTarget: string | null;
}

export interface CompareEntry {
  /** Pfad relativ zu den verglichenen Ordnern, mit `/` getrennt. Leer nur für nicht lesbare Wurzeln. */
  rel: string;
  state: CompareState;
  left: CompareSide | null;
  right: CompareSide | null;
  /** Grund bei `unknown`. */
  note: string | null;
  /** Ordner: der Inhalt wurde gelesen und steht im Ergebnis. */
  listed: boolean;
}

export type CompareCounts = Record<CompareState, number>;

export interface CompareSummary {
  left: string;
  right: string;
  mode: CompareMode;
  recursive: boolean;
  /** Alle Einträge des Ergebnisses (unabhängig vom Filter). */
  entries: number;
  counts: CompareCounts;
  /** Etwas wurde nicht untersucht: ein Abgleich daraus löscht nie etwas. */
  incomplete: boolean;
  cancelled: boolean;
  issues: string[];
}

export interface CompareFilter {
  /** Nur diese Zustände; leer oder fehlend: alle. */
  states?: CompareState[] | null;
  /** Teil des relativen Pfads, Groß-/Kleinschreibung egal. */
  text?: string | null;
}

export interface ComparePage {
  id: number;
  offset: number;
  /** Treffer des Filters. */
  total: number;
  entries: CompareEntry[];
  summary: CompareSummary;
}

// ---------------------------------------------------------------- Suche über mehrere Dateien und Archive

/** `glob`: `*`, `?`, `[abc]`, `[!a-z]`, mehrere Muster mit `;`, passt auf den ganzen Namen. `regex`: Rust-`regex` (linear), passt irgendwo im Namen. */
export type SearchNameMode = "glob" | "regex";

/**
 * Optionen von `searchStart`; alles Fehlende gilt als Standard. Mindestens `name` oder `text` muss
 * gesetzt (und nicht leer) sein. Die Grenzen (Treffer, Dateien, Bytes, Zeit, Archivbudget) legt der
 * Kern fest, die Oberfläche kann sie nicht ändern.
 */
export interface SearchOptions {
  /** Namensmuster; leer oder fehlend: jeder Name. */
  name?: string | null;
  /** Standard `glob`. */
  nameMode?: SearchNameMode;
  /** Standard `false` (Groß-/Kleinschreibung egal). */
  nameCaseSensitive?: boolean;
  /** Text in Dateien (nur Textdateien: eine Datei mit NUL in den ersten 8 KiB zählt als binär und wird übersprungen). */
  text?: string | null;
  /** Der Text ist ein regulärer Ausdruck (sonst wörtlich). */
  textRegex?: boolean;
  textCaseSensitive?: boolean;
  minSize?: number | null;
  maxSize?: number | null;
  /** Millisekunden seit 1970; Dateien ohne Änderungszeit passen nie auf einen Datumsfilter. */
  modifiedAfter?: number | null;
  modifiedBefore?: number | null;
  /** Standard `true`. */
  recursive?: boolean;
  /** In Archive hineinsuchen (mit den Archivgrenzen des Kerns). Standard `false`. */
  inArchives?: boolean;
  /** Archivebenen (1 bis 3), Standard 1. */
  archiveDepth?: number;
  /** Server und Plugin-Dateisysteme nur bei ausdrücklicher Wahl, dann mit kleinem Budget. Standard `false`. */
  includeRemote?: boolean;
}

export interface SearchHit {
  /** Ort des Treffers (Archivpfad im IPC-Format), mit `list` im übergeordneten Ordner zu öffnen. */
  path: string;
  kind: EntryKind;
  size: number;
  /** Textfund: Zeilennummer ab 1; Namensfund: `null`. */
  line: number | null;
  /** Gekürzter Zeilenauszug (nur Text, höchstens `MAX_SEARCH_EXCERPT_CHARS` Zeichen plus Auslassungszeichen). */
  excerpt: string | null;
  /** Lage und Länge des Treffers im Auszug, in Zeichen. */
  matchStart: number;
  matchLen: number;
}

/** Was die Suche vorzeitig beendet hat. */
export type SearchStopReason = "hits" | "files" | "bytes" | "time" | "cancelled";

export interface SearchSummary {
  root: string;
  /** Alle Treffer des Ergebnisses. */
  total: number;
  dirsVisited: number;
  filesVisited: number;
  /** Dateien, deren Inhalt gelesen wurde. */
  filesSearched: number;
  archivesSearched: number;
  skippedBinary: number;
  /** Pipes, Geräte, Sockets (nie gelesen). */
  skippedSpecial: number;
  bytesRead: number;
  /** Etwas wurde nicht untersucht (Grenze, Lesefehler, Abbruch): nie still abgeschnitten, Gründe in `issues`. */
  incomplete: boolean;
  cancelled: boolean;
  stopped: SearchStopReason | null;
  issues: string[];
}

export interface SearchPage {
  id: number;
  offset: number;
  total: number;
  hits: SearchHit[];
  summary: SearchSummary;
}

export type SyncMode = "leftToRight" | "rightToLeft" | "mirror";

export interface SyncOptions {
  /** `false`: nur Fehlendes ergänzen. `true`: auch Neueres ersetzen (Spiegeln: auch Abweichendes). */
  replaceNewer: boolean;
}

/** `copy`: Ziel ist neu; `replace`: Ziel wird überschrieben; `trash`: Ziel geht in den Papierkorb. */
export type SyncActionKind = "makeDir" | "copy" | "replace" | "trash";

export interface SyncAction {
  /** Stelle im Plan. */
  index: number;
  kind: SyncActionKind;
  /** Pfad des betroffenen Eintrags relativ zum Zielordner (so wählt `planSync(…, selection)` aus). */
  rel: string;
  entryKind: EntryKind;
  size: number;
}

export interface SyncPlanNote {
  rel: string;
  reason: string;
}

export interface SyncPlan {
  token: string;
  mode: SyncMode;
  source: string;
  target: string;
  makeDirs: number;
  copies: number;
  replaces: number;
  trashes: number;
  bytes: number;
  totalActions: number;
  /** Die ersten `SYNC_PLAN_PREVIEW` Aktionen; den Rest liefert `syncPlanPage`. */
  actions: SyncAction[];
  actionsTruncated: boolean;
  notes: SyncPlanNote[];
  notesTotal: number;
  /** Der Vergleich war unvollständig: es ist nichts zum Löschen geplant. */
  incomplete: boolean;
}

export interface SyncPlanPage {
  total: number;
  offset: number;
  actions: SyncAction[];
}

export interface DiffOptions {
  ignoreWhitespace: boolean;
  ignoreCase: boolean;
}

export type DiffHunkKind = "equal" | "delete" | "insert" | "replace";

/** Zeilen ohne `\n`, Zählung ab 0. Die Abschnitte decken beide Dateien lückenlos und in Reihenfolge ab. */
export interface DiffHunk {
  kind: DiffHunkKind;
  leftStart: number;
  left: string[];
  rightStart: number;
  right: string[];
}

export interface DiffResult {
  hunks: DiffHunk[];
  /** Abschnitte, die nicht `equal` sind. */
  differences: number;
  leftLines: number;
  rightLines: number;
}

/** Grenzen, die der Kern erzwingt (`state/compare_impl.rs`, `compare.rs`); der Mock hält dieselben ein. */
export const MAX_COMPARES = 4;
export const MAX_SYNC_PLANS = 8;
export const MAX_COMPARE_PAGE = 1000;
export const SYNC_PLAN_PREVIEW = 200;
export const MAX_COMPARE_FILTER_BYTES = 1024;
/** Grenzen der Suche (`search.rs`); der Mock hält dieselben ein. */
export const MAX_SEARCHES = 4;
export const MAX_SEARCH_PAGE = 500;
export const MAX_SEARCH_HITS = 10_000;
export const MAX_SEARCH_PATTERN_BYTES = 1024;
export const MAX_SEARCH_EXCERPT_CHARS = 200;
export const DIFF_MAX_BYTES = 2 * 1024 * 1024;
export const DIFF_MAX_LINES = 50_000;
/** Meldungen des Textvergleichs (Fehlerart `unsupported`), wörtlich wie im Kern. */
export const MSG_DIFF_BINARY = "Binärdateien können nicht als Text verglichen werden";
export const MSG_DIFF_TOO_LARGE = "Die Datei ist zu groß für den Textvergleich";

export interface Bookmark {
  name: string;
  path: string;
  color: string;
  order: number;
}

export interface TabState {
  path: string;
  sort: string;
  filter: string;
  /** When set, the tab is locked to this folder: it cannot navigate above it. */
  lockRoot?: string;
}

export interface WindowSession {
  tabs: TabState[];
  activeTab: number;
}

export type VerifyPolicy = "never" | "auto" | "always";

export interface Settings {
  sidebarMode?: "hidden" | "icons" | "full";
  sidebarWidth?: number;
  sidebarCollapsed?: string[];
  sidebarHidden?: string[];
  sidebarDiscovery?: boolean;
  verifyPolicy: VerifyPolicy;
  version: number;
  theme: "schiefer" | "papier" | "werkstatt" | "weiss";
  accent: string;
  accentAutomatic?: boolean;
  showParentEntry?: boolean;
  /** Höhe (px) des eingebetteten Terminals; fehlt der Wert, gilt 260. */
  terminalHeight?: number;
  /** Shell des eingebetteten Terminals: "auto" (fish mit Vorschlägen, sonst bash/sh), "bash" (immer schlichte bash/sh) oder "login" (eigene $SHELL mit Konfiguration). Fehlt der Wert, gilt "login". */
  terminalShell?: "auto" | "bash" | "login";
  /** Farben des Terminals: "theme" (aus dem Design), "classic", "green", "amber", "light" oder "custom" (terminalBg/terminalFg). Fehlt der Wert, gilt "green". */
  terminalColors?: "theme" | "classic" | "green" | "amber" | "light" | "custom";
  /** Nur bei terminalColors "custom": #rrggbb. */
  terminalBg?: string;
  terminalFg?: string;
  extensionColumn?: boolean;
  showPermissionsColumn?: boolean;
  showUidColumn?: boolean;
  showGidColumn?: boolean;
  paneTintStrength?: number;
  /** Breiten (px) der drei festen Listenspalten je Modus; fehlt ein Modus, gelten die Standardbreiten. */
  ownerColumnWidths?: { normal?: [number, number, number]; ext?: [number, number, number] };
  columnOrder?: { normal?: import("../state/columns").ColumnKey[]; ext?: import("../state/columns").ColumnKey[] };
  columnWidths?: { normal?: [number, number, number]; ext?: [number, number, number] };
  /** Optional tint (hex) per window; missing/empty = no tint. */
  paneColorLeft?: string;
  paneColorRight?: string;
  /** Umbelegte Tastenkürzel: Befehl → Spec (z. B. "F9", "Ctrl+Shift+G"); fehlt ein Befehl, gilt die Standardtaste. */
  keybindings?: Record<string, string>;
  checkUpdatesOnStart?: boolean;
  trayEnabled?: boolean;
  startMenuPrompted?: boolean;
  startMenuPromptCount?: number;
  /** The one-time "start with the system?" question was answered; the mode lives in the autostart entry. */
  autostartPrompted?: boolean;
  dismissedUpdateVersion?: string;
  lastUpdateCheck?: string | null;
  lastSeenVersion?: string | null;

  density: "compact" | "normal" | "comfortable";
  fontSize: number;
  language: "de" | "en" | "auto";
  bookmarks: Bookmark[];
  session: { windows: WindowSession[]; activeWindow: number };
}

export type LoadOutcome =
  | { kind: "fresh" }
  | { kind: "loaded" }
  | { kind: "repaired"; messages: string[] }
  | { kind: "recoveredFromCorrupt" }
  | { kind: "newerVersion"; version: number }
  | { kind: "migrated"; from: number; to: number };

/** Ereignisse vom Kern an die Oberfläche. */
export type ItemErrorDecision = "skip" | "retry" | "abort";
export interface ItemErrorInfo { jobId: number; name: string; kind: "permission" | "verifyFailed" | "io" | "disconnected"; message: string }

export type CoreEvent =
  | { type: "placesChanged" }
  | { type: "discoveryChanged" }
  | { type: "closeSaveRequested" }
  | { type: "closeRequested"; count: number }
  | { type: "itemError"; error: ItemErrorInfo }
  | { type: "job"; status: JobStatus }
  | { type: "conflict"; conflict: ConflictInfo }
  | { type: "dirChanged"; path: string }
  | { type: "connectionsChanged" };

/** Look of the main window, handed to the connection editor window (it never reads the settings file). */
export interface EditorAppearance {
  theme: Settings["theme"];
  accent: string;
  accentAutomatic: boolean;
  density: Settings["density"];
  fontSize: number;
  lang: "de" | "en";
}

// ---------------------------------------------------------------- Server-Verbindungen (FTP/FTPS/SFTP)

export type ConnectionProtocol = "ftp" | "ftps" | "sftp" | "smb" | "nfs" | "webdav" | "system";
/** ftp => "none", ftps => "explicit" | "implicit", sftp => "none". */
export type TlsModeKind = "none" | "explicit" | "implicit";
export type SftpAuthMethod = { kind: "password" } | { kind: "keyFile"; path: string };

/** Gemerkter Fingerabdruck (SHA-256) eines Zertifikats (FTPS/WebDAV) bzw. Serverschlüssels (SFTP). */
export interface PinInfo {
  protocol: "ftps" | "sftp" | "webdav";
  host: string;
  port: number;
  sha256: string;
  keyType: string | null;
}

/** Verbindungsprofil. Enthält nie ein Passwort; das geht nur über `connectionConnect`/`secretSet`. */
export interface ConnectionProfile {
  share?: string;
  domain?: string;
  guest?: boolean;
  export?: string;
  uid?: number;
  gid?: number;
  systemUrl?: string | null;
  /** Missing on legacy profiles; zero port/empty user inherit OpenSSH configuration. */
  transport?: "builtin" | "openssh";
  /** Leer bei einem neuen Profil; `connectionSave` vergibt die Kennung (`[a-z0-9-]`, höchstens 32 Zeichen). */
  id: string;
  name: string;
  protocol: ConnectionProtocol;
  host: string;
  port: number;
  user: string;
  startFolder: string | null;
  tls: TlsModeKind;
  /** Nur bei `ftp` (ohne TLS) relevant: ausdrückliche Erlaubnis, Passwort und Daten unverschlüsselt zu senden. */
  allowPlaintext: boolean;
  auth: SftpAuthMethod;
  saveSecret: boolean;
}

export type SecretStoreKind = "keychain" | "session" | "none";

export interface ConnectionEntry {
  profile: ConnectionProfile;
  /** Nur lesend; `connectionSave` ignoriert sie. */
  pins: PinInfo[];
  connected: boolean;
  connectionId: string | null;
  /** `ftp:`/`sftp:`-Pfad des Startordners der aktuellen Verbindung. */
  root: string | null;
  /** Nur „gesetzt / nicht gesetzt“, nie das Geheimnis selbst. */
  secretSet: boolean;
  secretStore: SecretStoreKind;
}

export type TrustKind = "unknownIssuer" | "hostnameMismatch" | "newHostKey";

export type ConnectOutcome =
  | { state: "connected"; connectionId: string; root: string; secretStore: SecretStoreKind }
  | { state: "needsTrust"; fingerprint: string; keyType: string | null; kind: TrustKind; changed: false }
  /** Nur Anzeige: bei abweichendem Pin wird nie verbunden. */
  | { state: "pinMismatch"; old: string; new: string };

export type TrustDecision = "once" | "always";

// ---------------------------------------------------------------- Plugins

/** Art eines Plugins; ein Plugin kann mehrere Arten haben. */
export type PluginKind = "filesystem" | "packer" | "column" | "preview";

/**
 * Zustand eines Plugins. `new`: noch nie bestätigt; `locked`: das Paket wurde seit der Bestätigung
 * verändert (erst `pluginTrustReset`, dann ist es wieder `new`); `sandboxUnavailable`: ohne Sandbox
 * nicht startbar; `failed`: zu oft ausgefallen.
 */
export type PluginStatus = "new" | "enabled" | "disabled" | "locked" | "sandboxUnavailable" | "failed";

/** Ergebnis der Sandbox-Prüfung (Landlock) für ein Plugin. */
export interface PluginSandboxProbe {
  available: boolean;
  landlockAbi: number;
  landlockNet: boolean;
  landlockScoped: boolean;
}

export interface PluginColumn {
  /** `<pluginId>:<spaltenId>`, Ids aus `[a-z0-9-]`. */
  id: string;
  title: string;
}

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  kinds: PluginKind[];
  status: PluginStatus;
  /** Das Manifest will schreiben dürfen. */
  wantsWrite: boolean;
  /** Der Nutzer hat das Schreiben erlaubt (nie ohne `wantsWrite`). */
  writeGranted: boolean;
  /** Der Nutzer hat „ohne Sandbox“ bestätigt. */
  unsandboxed: boolean;
  enabled: boolean;
  /** SHA-256 der `plugin.json` (64 Hexziffern); leer, wenn das Paket nicht lesbar ist. */
  manifestHash: string;
  sandbox: PluginSandboxProbe;
  failures: number;
  disabledReason: "failures" | "locked" | null;
  /** Feste, bereinigte Kurzmeldung bei `locked`/`failed`/Paketfehler. */
  problem: string | null;
  /** Bereinigter Text, höchstens `MAX_PLUGIN_DIAGNOSTIC_CHARS` Zeichen, nur Text (nie HTML). */
  diagnostic: string;
  columns: PluginColumn[];
  /** Kleingeschrieben, ohne Punkt. */
  packerExtensions: string[];
  /** `plugin:<id>!/`, wenn das Plugin aktiv ist und die Art `filesystem` hat. */
  filesystemRoot: string | null;
}

/** Werte einer Spalte; `values[i]` gehört zu `paths[i]`, `null` = kein Wert (z. B. Ordner). */
export interface PluginColumnValues {
  columnId: string;
  values: (string | null)[];
}

/** Vorschau eines Plugins: nur Text oder ein Rasterbild (nie HTML oder SVG). `data` ist Base64. */
export type PluginPreview =
  | { kind: "text"; text: string }
  | { kind: "image"; mime: "image/png" | "image/jpeg" | "image/gif" | "image/webp"; data: string };

/** Grenzen, die der Kern erzwingt; der Mock hält dieselben ein. */
export const MAX_PLUGIN_COLUMN_PATHS = 500;
export const MAX_PLUGIN_COLUMN_IDS = 8;
export const MAX_PLUGIN_NAME_CHARS = 64;
export const MAX_PLUGIN_COLUMN_TITLE_CHARS = 40;
export const MAX_PLUGIN_DIAGNOSTIC_CHARS = 512;
export const MAX_PLUGIN_CELL_CHARS = 256;
export const MAX_PLUGIN_PREVIEW_TEXT_CHARS = 256 * 1024;

export type UndoErrorKind = "changed" | "conflict" | "alreadyUndone" | "notFound" | "unsupported" | "notLocal" | "io";
export type UndoKind = "move" | "rename" | "trash" | "copy" | "mkdir";
export interface UndoEntry { first?: string; more?: number; id: number; time: number; label: string; kind: UndoKind; undone: boolean }
export interface UndoResult { jobId: number; ok: boolean; done: number; remaining: number; cancelled: boolean; errors: UndoErrorKind[] }
/** Transient OpenSSH question; the answer never travels in an event. */
export interface SshPrompt {
  id: string;
  kind: "password" | "passphrase" | "hostKey" | "other";
  text: string;
  secret: boolean;
}

export interface SshResolvedHost {
  hostname: string;
  user: string;
  port: number;
  identityfile: string[];
}

/** Local scan sessions, bounded to two per kind including running jobs. */
export interface ScanProgress { stage: "scan" | "hash" | "group" | "size" | "partialHash" | "fullHash" | ""; entries: number; bytes: number; total: number | null }
export interface UsageTotals { size: number; allocated: number; files: number; directories: number }
export interface UsageEntry { path: string; totals: UsageTotals }
export interface UsagePage { ready: boolean; progress: ScanProgress; seconds: number; path: string; totals: UsageTotals | null; children: UsageEntry[]; other: UsageTotals | null; issues: [string, string][]; skippedCount: number; incomplete: boolean }
export type KeepRule = "oldest" | "newest" | "shortestPath" | "inFolder";
export interface KeepChoice { groupIndex: number; keepPath: string }
export interface DupeEntry { path: string; modified: number | null; protected: boolean }
export interface DupeGroup { groupIndex: number; size: number; reclaimableBytes: number; entries: DupeEntry[] }
export interface DupesPage { cancelled?: boolean; ready: boolean; progress: ScanProgress; total: number; reclaimableBytes: number; groups: DupeGroup[]; incomplete: boolean; issues: [string,string][] }
export interface DupesTrashResult { jobId: number; skipped: { path: string; reason: string }[] }

export interface KeepSuggestion extends KeepChoice { redundantCount: number; redundantBytes: number }

export type SimilarRule = "largestResolution" | "largestFile" | "oldest" | "newest" | "shortestPath";
export interface SimilarEntry extends DupeEntry { size: number; width: number; height: number; hash: string; distanceToReference: number }
export interface SimilarGroup { groupIndex: number; reclaimableBytes: number; entries: SimilarEntry[] }
export interface SimilarPage extends Omit<DupesPage, "groups"> { groups: SimilarGroup[]; images: number; cancelled: boolean }
export interface SimilarChoice { groupIndex: number; trashPaths: string[] }
export interface Thumbnail { bytes: number[]; width: number; height: number }

/** A locally mounted removable drive (USB stick, external disk) under /media or /run/media. */
export interface Drive {
  name: string;
  path: string;
  device?: string;
  fstype?: string;
  kind?: "root" | "partition" | "removable" | "network" | "fuse" | "automount";
  readOnly?: boolean;
  source?: "mount" | "fstab" | "device";
  mounted?: boolean;
  deviceActions?: { key: string; removable: boolean; encrypted: boolean; optical: boolean } | null;
  fstab?: { permission?: "user" | "users" | "owner" | null; userMountable: boolean; automount: boolean; noauto: boolean } | null;
  space?: { available: number; total: number } | null;
}
export interface UserDir { kind: "desktop" | "documents" | "downloads" | "music" | "pictures" | "videos"; path: string }
export interface DiscoveredService {
  key: string; name: string; protocol: "smb" | "nfs" | "sftp" | "webdav" | "webdavs";
  host: string; port: number; path: string | null;
}
export interface PlacesSnapshot {
  discoveredServices?: DiscoveredService[];
  home: string;
  userDirs: UserDir[];
  trash: { path: string; empty: boolean };
  drives: Drive[];
  networkMounts: SystemPlace[];
  /** Desktop network bookmarks (Dolphin, GNOME Files) that are not mounted. */
  systemPlaces?: SystemPlace[];
}

export interface SystemPlace {
  /** Opaque, stable key of a desktop bookmark in the sidebar (never the URL). */
  placeKey?: string;
  mountKey?: string;
  canUnmount?: boolean;
  sources: string[];
  name: string;
  scheme: string;
  host: string;
  port: number | null;
  user: string | null;
  path: string;
  url: string;
  mountedAt: string | null;
}

export type SettingsTab = "allgemein" | "darstellung" | "dateiliste" | "seitenleiste" | "terminal" | "kopieren" | "tastatur" | "info" | "appearance" | "system" | "keyboard";
export type SettingsWindowSettings = Pick<Settings, "sidebarMode" | "sidebarWidth" | "sidebarCollapsed" | "sidebarHidden" | "sidebarDiscovery" | "theme" | "accent" | "accentAutomatic" | "density" | "fontSize" | "language" | "showParentEntry" | "paneTintStrength" | "showPermissionsColumn" | "showUidColumn" | "showGidColumn" | "extensionColumn" | "paneColorLeft" | "paneColorRight" | "terminalColors" | "terminalBg" | "terminalFg" | "verifyPolicy" | "terminalShell" | "trayEnabled" | "checkUpdatesOnStart" | "lastUpdateCheck" | "keybindings">;
export type SettingsWindowPatch = Partial<Omit<SettingsWindowSettings, "lastUpdateCheck">>;
export type SettingsWindowMessage = { session: string; seq: number } & (
  { kind: "patch"; patch: SettingsWindowPatch } | { kind: "checkUpdates" } |
  { kind: "setTray"; enabled: boolean } | { kind: "setStartMenu"; enabled: boolean } |
  { kind: "setAutostart"; mode: import("./system").AutostartMode } |
  { kind: "refreshStatus" } | { kind: "session" } | { kind: "closed" } | { kind: "closeFailed" } | { kind: "flush" });
export interface SettingsWindowSnapshot {
  session: string;
  ack: number;
  errors: { seq: number; message: string }[];
  settings: SettingsWindowSettings;
  system: Pick<import("../app/system").SystemState, "status" | "check" | "error" | "installing" | "trayUnavailable">;
  logDir: string;
  persistence: { outcome: LoadOutcome; error: string };
}
export type SettingsWindowDelivery = { kind: "session"; session: string } |
  { kind: "close"; session: string } | { kind: "timeout"; session: string } | { kind: "error"; session: string; message: string } | { kind: "snapshot"; snapshot: SettingsWindowSnapshot };

/** One item in the trash view (`trashList`); `id` is the stable identifier used for restore and purge. */
export interface TrashEntry {
  id: string;
  name: string;
  originalParent: string;
  /** Unix seconds, null if unknown. */
  deletedAt: number | null;
  /** Bytes of a file; null for folders and when unknown. */
  size: number | null;
  isDir: boolean;
}

export interface TrashList {
  entries: TrashEntry[];
  /** More items exist than the cap; only the newest are listed. */
  truncated: boolean;
}
