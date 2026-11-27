import type { SettingsTab, SettingsWindowMessage, SettingsWindowSnapshot, SettingsWindowDelivery, WindowSize } from "./types";
import type { SystemApi } from "./system";
import type { SimilarRule, SimilarPage, SimilarChoice, Thumbnail } from "./types";
import type {
  KeepSuggestion, UsagePage, DupesPage, KeepChoice, KeepRule, DupesTrashResult,
  UndoEntry,
  UndoResult,
  SshPrompt,
  SshResolvedHost,
  CompareFilter,
  ConnectOutcome,
  ConnectionEntry,
  EditorAppearance,
  ConnectionProfile,
  CompareOptions,
  ComparePage,
  ConflictAnswer,
  CoreEvent,
  DeletePlan,
  DiffOptions,
  DiffResult,
  FileInfo,
  JobStatus,
  Listing,
  PackFormat,
  LoadOutcome,
  PluginColumnValues,
  PluginInfo,
  PluginPreview,
  RegexResult,
  RenameItem,
  SearchOptions,
  SearchPage,
  SecretStoreKind,
  SearchDirection,
  SearchResult,
  Settings,
  SyncMode,
  SyncOptions,
  SyncPlan,
  SyncPlanPage,
  TrustDecision,
} from "./types";

/**
 * Fehler eines Befehls: klare deutsche Meldung vom Kern, ohne Interna. Arten (`kind`) unter
 * anderem `notFound`, `exists`, `permission`, `invalidPath`, `unsupported`, `io` sowie für
 * Plugins `pluginLocked`, `pluginSandboxUnavailable`, `pluginTimeout`, `pluginProtocol`,
 * `pluginDisabled`, `pluginCrashed`, `pluginTooLarge`, `pluginNotInstalled`, `pluginUnsafe`,
 * `pluginBusy`, `pluginFailed` (feste Texte, nie Text des Plugins) sowie für
 * Archive `readOnly` (Schreiben in ein Archiv: immer abgelehnt) und `archive` (beschädigt,
 * verschlüsselt, zu groß oder zu langsam für die Anzeige).
 */
export class CoreError extends Error {
  constructor(
    message: string,
    public readonly kind: string = "io",
  ) {
    super(message);
  }
}

/**
 * Einzige Schnittstelle zum Dateisystem. In der App (`createTauriApi`) von Tauri-Befehlen
 * getragen, in Tests und im Browser-Entwurf von `createMockApi` ersetzt.
 */
export interface TerminalOutput { id: number; data: string }
export interface TerminalExit { id: number; code: number | null }

export interface NativeDropEvent { type: "over" | "drop" | "leave"; paths: string[]; x: number; y: number }

export interface DualfileApi extends SystemApi {
  terminalOpen(cwd: string | undefined, rows: number, cols: number): Promise<number>;
  terminalWrite(id: number, data: string): Promise<void>;
  terminalResize(id: number, rows: number, cols: number): Promise<void>;
  terminalClose(id: number): Promise<void>;
  onTerminalOutput(handler: (event: TerminalOutput) => void): () => void;
  onTerminalExit(handler: (event: TerminalExit) => void): () => void;
  spaceFree(path: string): Promise<{ available: number; total: number } | null>;
  usageStart(path: string, options: { oneFileSystem: boolean }): Promise<number>;
  usageNode(id: number, nodePath?: string): Promise<UsagePage>;
  usageClose(id: number): Promise<void>;
  similarStart(roots: string[], options: {minSize: number; maxDistance: number}): Promise<number>;
  similarPage(id: number, offset: number, limit: number): Promise<SimilarPage>;
  similarClose(id: number): Promise<void>;
  similarSuggest(id: number, rule: SimilarRule): Promise<KeepChoice[]>;
  similarThumbnail(id: number, groupIndex: number, entryIndex: number): Promise<Thumbnail>;
  similarTrash(id: number, choices: SimilarChoice[]): Promise<DupesTrashResult>;
  dupesStart(roots: string[], options: { minSize: number }): Promise<number>;
  dupesPage(id: number, offset: number, limit: number): Promise<DupesPage>;
  dupesClose(id: number): Promise<void>;
  dupesSuggest(id: number, rule: KeepRule, folder: string): Promise<KeepSuggestion[]>;
  dupesTrash(id: number, choices: KeepChoice[]): Promise<DupesTrashResult>;

  undoList(): Promise<UndoEntry[]>;
  /** Runs a cancellable job; its ID arrives through onEvent before this resolves. */
  undoRun(id: number): Promise<UndoResult>;
  onUndoChanged(handler: () => void): () => void;
  /** Files dragged in from outside (file manager, desktop); positions are CSS pixels relative to the window. */
  onNativeDrop(handler: (event: NativeDropEvent) => void): () => void;
  discoverySnapshot(): Promise<import("./types").DiscoveredService[]>;
  placesSnapshot(): Promise<import("./types").PlacesSnapshot>;
  placesSpace(path: string): Promise<{ available: number; total: number } | null>;
  placesSetVisible(visible: boolean): Promise<void>;
  trashOpenSystem(): Promise<void>;
  placesMountAffected(mountKey: string, paths: string[]): Promise<number[]>;
  placesSaveConnection(key: string): Promise<void>;
  trashEmpty(): Promise<import("./types").TrashEmptyStarted>;
  /** Trash view: items of the local trash (newest first, capped). */
  trashList(): Promise<import("./types").TrashList>;
  /** Restores by id to the original locations (`exists` if occupied); returns the restored original paths. */
  trashRestore(ids: string[]): Promise<string[]>;
  /** Deletes by id for good; call only after the user confirmed. Returns the number of deleted items. */
  trashPurge(ids: string[]): Promise<number>;
  deviceList(): Promise<import("./types").Drive[]>;
  deviceMount(deviceKey: string): Promise<string>;
  deviceUnmount(deviceKey: string): Promise<void>;
  deviceEject(deviceKey: string): Promise<void>;
  devicePowerOff(deviceKey: string): Promise<void>;
  fstabMount(mountPoint: string): Promise<void>;
  fstabUnmount(mountPoint: string): Promise<void>;
  placesMountUnmount(mountKey: string): Promise<void>;
  sidebarResizeWindow(delta: number): Promise<{ actualDelta: number; canGrow: boolean; restoredWidth?: number }>;
  localDrives(): Promise<import("./types").Drive[]>;
  systemPlacesList(): Promise<import("./types").SystemPlace[]>;
  systemPlacesImport(urls: string[]): Promise<ConnectionEntry[]>;
  systemMountConnect(profileId: string): Promise<string>;
  systemMountUnmount(profileId: string): Promise<void>;
  sshHostsList(): Promise<string[]>;
  sshHostResolve(alias: string): Promise<SshResolvedHost>;
  onSshPrompt(handler: (prompt: SshPrompt) => void): () => void;
  sshPromptAnswer(id: string, text: string | null): Promise<void>;
  /**
   * Pfade sind lokal (`/abs/pfad`) oder liegen in einem Archiv: `archive:<Archivdatei>!<innen>`,
   * in der Archivdatei sind `%` und `!` als `%25`/`%21` kodiert, der Innenpfad beginnt mit `/`
   * (Beispiel `archive:/home/u/a%21b.zip!/ordner/x.txt`, verschachtelt `archive:archive:/a.zip%21/in.tar!/x`).
   * Archive sind schreibgeschützt; Kopieren AUS dem Archiv (`copy`) entpackt.
   */
  list(path: string): Promise<Listing>;
  homeDir(): Promise<string>;
  copy(sources: string[], destination: string): Promise<number>; // Auftragsnummer
  /** „Hier duplizieren“: legt je Quelle eine Kopie im selben Ordner unter freiem Namen an (`name_copy`, `name_copy2`, …), ohne Rückfrage. */
  duplicate(sources: string[]): Promise<number>;
  move(sources: string[], destination: string): Promise<number>;
  /**
   * Packt die Quellen (lokal oder aus einem Archiv, alle aus demselben Ort) als Auftrag in ein
   * NEUES Archiv `name` im lokalen Ordner `destination` (Auftragsnummer). Ein vorhandener Name wird
   * nie ersetzt: `exists`; „Beide behalten“ macht die Oberfläche über einen neuen Namen. Das Ziel
   * innerhalb einer Quelle scheitert im Auftrag; Abbruch hinterlässt nichts.
   */
  packArchive(sources: string[], destination: string, name: string, format: PackFormat): Promise<number>;
  trash(items: string[]): Promise<number>;
  restoreFromTrash(items: string[]): Promise<void>;
  /** Endgültiges Löschen ist zweistufig: erst planen, dann mit dem Token bestätigen. */
  planDelete(items: string[]): Promise<DeletePlan>;
  deletePermanent(plan: DeletePlan, confirmedToken: string): Promise<number>;
  rename(path: string, newName: string): Promise<{ oldPath: string; newPath: string }>;
  /**
   * Benennt mehrere Einträge EINES Ordners um (Auftrag, Auftragsnummer). Der Kern prüft alles
   * erneut: Namen, doppelte Ziele und Kollisionen mit nicht umbenannten Einträgen lehnen den
   * ganzen Lauf ab (nichts geändert, Auftrag `failed`); eine fehlende Quelle scheitert allein.
   * Tausch, Ketten und Zyklen funktionieren; nie wird etwas überschrieben. Im Auftrag stehen
   * fehlgeschlagene Einträge (`failed`) mit ihrem ALTEN Pfad; alle übrigen sind umbenannt.
   */
  renameMany(folder: string, items: RenameItem[]): Promise<number>;
  /**
   * Ersetzt in Namen per regulärem Ausdruck (Rust-`regex`, lineare Laufzeit, Syntax wie RE2:
   * `$1`/`${name}`, keine Rückverweise, keine Lookarounds). Ein unbrauchbares Muster oder zu
   * viele Namen (siehe `MAX_REGEX_*`) lässt den Aufruf mit `invalidPattern`/`invalidInput` scheitern;
   * ein einzelner zu langer Name nur seinen Eintrag.
   */
  regexReplace(
    names: string[],
    pattern: string,
    replacement: string,
    ignoreCase: boolean,
    all: boolean,
  ): Promise<RegexResult[]>;
  makeDir(parent: string, name: string): Promise<string>;
  /** Legt eine leere Datei an; ein vorhandener Name wird nie überschrieben (`exists`). */
  makeFile(parent: string, name: string): Promise<string>;
  openExternal(path: string): Promise<void>; // nur xdg-open
  /** Öffnet eine https-Adresse im Standardbrowser. */
  openUrl(url: string): Promise<void>;
  logInfo(): Promise<import("./types").LogInfo>;
  /** Letzte 500 Zeilen der Logdatei, auf Wunsch anonymisiert. */
  logRead(anonymize: boolean): Promise<string>;
  /** Schreibt das Log in einen lokalen Ordner und liefert den vollen Pfad der neuen Datei. */
  logSave(dir: string, anonymize: boolean): Promise<string>;
  logOpenFolder(): Promise<void>;
  /** https-Adresse eines vorbefüllten GitHub-Issues. */
  reportIssueUrl(): Promise<string>;
  /** Nur zum Protokollieren; scheitert nie sichtbar. */
  logFrontend(level: import("./types").LogLevel, message: string): Promise<void>;
  appCloseNow(): Promise<void>;
  appConfirmClose(): Promise<void>;
  answerItemError(jobId: number, decision: import("./types").ItemErrorDecision): Promise<void>;
  jobPause(id: number): Promise<void>;
  jobResume(id: number): Promise<void>;
  cancelJob(id: number): Promise<void>;
  answerConflict(jobId: number, answer: ConflictAnswer): Promise<void>;
  jobs(): Promise<JobStatus[]>;
  loadSettings(): Promise<{ settings: Settings; outcome: LoadOutcome }>;
  saveSettings(settings: Settings): Promise<void>;
  /**
   * Meldet dem Kern die Ordner, deren Änderungen die Oberfläche sehen will (ersetzt die bisherige
   * Liste); danach kommen `dirChanged`-Ereignisse. Eine leere Liste beendet die Beobachtung.
   */
  watch(paths: string[]): Promise<void>;
  /** Größe, Art und Änderungszeit einer Datei für den Lister; folgt Links, nur reguläre Dateien. */
  fileInfo(path: string): Promise<FileInfo>;
  /** EXIF-/ID3-Metadaten der Dateien (nur lokale; sonst leere Felder) für die Umbenennen-Platzhalter. */
  mediaTags(paths: string[]): Promise<import("./types").MediaTags[]>;
  /**
   * Liest höchstens `len` Bytes ab `offset` (der Kern kürzt auf `MAX_BLOCK_BYTES`; am Dateiende
   * kürzer, dahinter leer). Rohe Bytes, die Dekodierung macht die Oberfläche.
   */
  readBlock(path: string, offset: number, len: number): Promise<Uint8Array>;
  /**
   * Sucht Bytes (`needle`, höchstens `MAX_NEEDLE_BYTES`) in der Datei. Vorwärts: erster Treffer
   * ab `start`; rückwärts: letzter Treffer, der vor `start` beginnt. Je Aufruf wird nur eine
   * begrenzte Strecke durchsucht (siehe `continue`). Groß-/Kleinschreibung nur für ASCII.
   * Ein Abbruch (`cancelSearch` oder eine neuere Suche) endet mit CoreError der Art `cancelled`.
   */
  findInFile(
    path: string,
    needle: Uint8Array,
    start: number,
    direction: SearchDirection,
    ignoreCase: boolean,
  ): Promise<SearchResult>;
  /** Bricht die laufende Suche ab; ohne laufende Suche wirkungslos. */
  cancelSearch(): Promise<void>;
  /**
   * Vergleicht zwei Ordner (lokal oder im Archiv, rekursiv nach Wunsch) als Auftrag mit Fortschritt
   * und Abbruch (Auftragsnummer). Die Nummer ist zugleich die Kennung des Ergebnisses: sobald der
   * Auftrag `done` ist, liest `comparePage` es seitenweise; `compareClose` gibt es frei. Es gibt
   * höchstens `MAX_COMPARES` Ergebnisse gleichzeitig (laufende zählen mit; sonst `invalidInput`), ein
   * abgebrochener Vergleich hinterlässt keines. Nicht lesbare Teile sind `unknown` und machen das
   * Ergebnis `incomplete`.
   */
  compareDirs(left: string, right: string, options: CompareOptions): Promise<number>;
  /** Eine Seite des Ergebnisses (höchstens `MAX_COMPARE_PAGE` Einträge) mit Filter und Gesamtübersicht. */
  comparePage(id: number, offset: number, limit: number, filter?: CompareFilter): Promise<ComparePage>;
  /** Gibt das Ergebnis und die daraus gebauten Pläne frei. */
  compareClose(id: number): Promise<void>;
  /**
   * Baut einen Abgleichsplan aus einem Ergebnis (ändert nichts) und merkt ihn unter seinem Token.
   * Archive sind nur Quelle: ein Archiv als Ziel scheitert mit `readOnly`. `selection` (Pfade relativ
   * zum Zielordner, wie `SyncAction.rel`) behält nur diese Aktionen samt der nötigen Ordner; ein
   * Eintrag, der nicht im Plan steht, lehnt den Aufruf ab. Löschen gibt es nur beim Spiegeln, nur in
   * den Papierkorb und nie bei unvollständigem Ergebnis.
   */
  planSync(compareId: number, mode: SyncMode, options: SyncOptions, selection?: string[]): Promise<SyncPlan>;
  /** Weitere Aktionen eines Plans (höchstens `MAX_COMPARE_PAGE`); der Plan bleibt gültig. */
  syncPlanPage(token: string, offset: number, limit: number): Promise<SyncPlanPage>;
  /**
   * Führt den bestätigten Plan als Auftrag aus (Auftragsnummer). Nur das Token zählt, jeder Plan gilt
   * einmal; ein falsches, fremdes oder verbrauchtes Token lehnt `invalidToken` ab. Vor jeder Aktion
   * wird das Ziel neu geprüft: Abweichung = übersprungen (`skipped`, Hinweis in `warnings`). Ein
   * geplantes Überschreiben fragt über die Konfliktbrücke (`conflict`-Ereignis, `answerConflict`).
   */
  runSync(token: string): Promise<number>;
  /**
   * Zeilenweiser Vergleich zweier Textdateien (Antwort begrenzt auf `DIFF_MAX_BYTES` und
   * `DIFF_MAX_LINES` je Seite). Binärdateien und zu große Dateien lehnt der Aufruf mit der Art
   * `unsupported` ab; die Meldung ist `MSG_DIFF_BINARY` bzw. `MSG_DIFF_TOO_LARGE`.
   */
  diffFiles(left: string, right: string, options: DiffOptions): Promise<DiffResult>;
  /**
   * Sucht unterhalb von `root` (Ordner lokal oder im Archiv, nur bei `includeRemote` auch auf Servern und in
   * Plugin-Dateisystemen) nach Namen und/oder Text als Auftrag mit Fortschritt und Abbruch (`cancelJob`;
   * Auftragsnummer). Die Nummer ist zugleich die Kennung des Ergebnisses: sobald der Auftrag `done` (oder
   * `cancelled`) ist, liest `searchPage` es seitenweise; `searchClose` gibt es frei. Es gibt höchstens
   * `MAX_SEARCHES` Ergebnisse gleichzeitig (laufende zählen mit; sonst `invalidInput`). Ein abgebrochener
   * Lauf behält seine bisherigen Treffer (`cancelled` und `incomplete`); wurde er schon vor dem Start
   * abgebrochen, gibt es kein Ergebnis. Links werden nie verfolgt, Pipes/Geräte nie gelesen, Archive nur mit
   * den Archivgrenzen. Fehlerarten beim Start: `invalidInput` (leere/ungültige/zu lange Muster, widersprüchliche
   * Filter), `invalidPath`, `notFound`, `notDirectory`, `unsupported` (Server ohne `includeRemote`), `disconnected`.
   */
  searchStart(root: string, options: SearchOptions): Promise<number>;
  /** Eine Seite des Ergebnisses (höchstens `MAX_SEARCH_PAGE` Treffer) mit Gesamtübersicht (`incomplete`, Gründe). */
  searchPage(id: number, offset: number, limit: number): Promise<SearchPage>;
  /** Gibt das Ergebnis frei. */
  searchClose(id: number): Promise<void>;
  /**
   * Server-Verbindungen. Serverorte sind `ftp:<verbindungs-id>!<pfad>` bzw. `sftp:<verbindungs-id>!<pfad>`
   * (Verbindungs-ID flüchtig, vom Kern vergeben, der Pfad steht wörtlich hinter dem ersten `!`). In
   * Einstellungen steht stattdessen `conn:<profil-id>!<pfad>`; ein nicht verbundenes Profil liefert dafür
   * CoreError `disconnected`, ein verbundenes löst auf seine aktuelle Verbindung auf. Ein Server hat keinen
   * Papierkorb (`trash`/`restoreFromTrash` → `unsupported`); Packen und Abgleichen schreiben nie auf Server.
   *
   * Alle Profile mit Pins und dem Stand „gesetzt/nicht gesetzt“ des Geheimnisses (nie das Geheimnis selbst).
   */
  connectionsList(): Promise<ConnectionEntry[]>;
  /**
   * Legt ein Profil an (`id` leer) oder ändert es; liefert den gespeicherten Eintrag. `pins` werden ignoriert.
   * Schaltet die Änderung `saveSecret` aus, wird das gespeicherte Geheimnis entfernt; lässt der Schlüsselbund das
   * nicht zu, scheitert der Aufruf mit `secretRemovalFailed` und das Profil bleibt unverändert.
   */
  connectionSave(profile: ConnectionProfile): Promise<ConnectionEntry>;
  /** Löscht das Profil samt Verbindung und gespeichertem Geheimnis. */
  connectionDelete(profileId: string): Promise<void>;
  /**
   * Verbindet. Passwort/Passphrase gibt es nur hier und nur einmal (`secret`). Ergebnis: verbunden,
   * Fingerabdruck muss bestätigt werden (`needsTrust`, dann `connectionTrust`) oder Pin-Abweichung
   * (`pinMismatch`, es wird nie verbunden). Fehler als CoreError: `auth`, `timeout`, `trust`,
   * `algorithmNotAllowed`, `remote`, `tooManyConnections`, `invalidInput`.
   * `expected` ist das Profil, wie die Oberfläche es zeigt: hat es sich inzwischen geändert (z. B. im
   * Editor-Fenster), scheitert der Aufruf mit `invalidInput`, statt zu einem anderen Ziel zu verbinden.
   */
  connectionConnect(profileId: string, secret?: string, expected?: ConnectionProfile): Promise<ConnectOutcome>;
  /**
   * Öffnet das Editor-Fenster für ein Verbindungsprofil (`null` = neues). `false`, wenn schon eins offen ist
   * (es kommt dann nach vorn). Nur vom Hauptfenster aus.
   */
  connectionEditorOpen(profileId: string | null, title: string, appearance: EditorAppearance, discoveryKey?: string): Promise<boolean>;
  /** Schließt das Editor-Fenster, aus dem der Aufruf kommt. */
  connectionEditorClose(): Promise<void>;
  /** `size`: remembered inner size (logical px) of the window, or null/omitted for the standard size. */
  settingsWindowOpen(tab: SettingsTab | null, title: string, appearance: EditorAppearance, size?: WindowSize | null): Promise<"opened" | "focused" | "inline">;
  /** Called by the settings window itself: is it maximized, fullscreen or minimized? (Its size then is not a size the user chose.) */
  settingsWindowZoomed(): Promise<boolean>;
  /** The settings window is asking the user something before it closes; the native close timeout waits. */
  settingsWindowHold(session: string, on: boolean): Promise<void>;
  settingsWindowClose(session: string, lastSeq: number, force?: boolean, cancel?: boolean): Promise<void>;
  settingsWindowAttach(handler: (delivery: SettingsWindowDelivery) => void): Promise<void>;
  settingsWindowSend(message: SettingsWindowMessage): Promise<void>;
  settingsWindowPush(snapshot: SettingsWindowSnapshot, title: string, close?: boolean, control?: { cancelClose?: boolean; error?: string }): Promise<boolean>;
  onSettingsWindowMessage(handler: (message: SettingsWindowMessage) => void): () => void;
  /**
   * Bestätigt NUR den zuletzt von `connectionConnect` gemeldeten Fingerabdruck und verbindet dann.
   * Anderer Fingerabdruck oder keine ausstehende Bestätigung: `invalidInput` (die Bestätigung ist danach verworfen).
   */
  connectionTrust(profileId: string, decision: TrustDecision, fingerprint: string): Promise<ConnectOutcome>;
  /** Löscht die Pins des Profils (bewusster eigener Schritt). */
  connectionResetTrust(profileId: string): Promise<void>;
  connectionDisconnect(profileId: string): Promise<void>;
  /** Merkt das Geheimnis: `keychain`, bei Schlüsselbund-Fehler `session` (nur im Speicher dieser Sitzung). */
  secretSet(profileId: string, secret: string): Promise<SecretStoreKind>;
  /** Entfernt das Geheimnis; geht das im Schlüsselbund nicht, `secretRemovalFailed`. */
  secretClear(profileId: string): Promise<void>;
  /**
   * Plugins. Ein aktives Plugin der Art `filesystem` hat einen Ort `plugin:<id>!/…` (Id aus `[a-z0-9-]`,
   * der Pfad steht wörtlich hinter dem ersten `!`, kein `..`; sonst `invalidPath`). Ist das Plugin nicht
   * aktiv (oder unbekannt), liefert jeder Zugriff darauf `disconnected`. Plugin-Orte sind lesbar wie
   * ein Server; Schreiben (Kopieren dorthin, Verschieben, Umbenennen, Anlegen, Löschen) lehnt `readOnly`
   * ab, solange das Plugin `writeGranted` nicht hat. Papierkorb und Packen/Abgleichen dorthin:
   * immer `unsupported`. Plugin-Texte kommen bereinigt an (Steuerzeichen → U+FFFD, Längen begrenzt).
   *
   * Alle bekannten Plugins mit Zustand (auch gesperrte und ausgefallene).
   */
  pluginsList(): Promise<PluginInfo[]>;
  /**
   * Aktiviert ein Plugin. `manifestHash` muss genau dem aktuellen Hash entsprechen (sonst `pluginLocked`);
   * ein gesperrtes Plugin (`locked`) wird nie aktiviert (erst `pluginTrustReset`). `unsandboxed` verlangt
   * `confirmUnsandboxed === true` (sonst `invalidInput`); ohne verfügbare Sandbox und ohne `unsandboxed`
   * scheitert der Aufruf mit `pluginSandboxUnavailable`. `grantWrite` gilt nur bei `wantsWrite`.
   * Aktivieren setzt `failures` auf 0. Unbekannte Id: `notFound`. Liefert den neuen Zustand.
   */
  pluginActivate(
    id: string,
    manifestHash: string,
    grantWrite: boolean,
    unsandboxed: boolean,
    confirmUnsandboxed: boolean,
  ): Promise<PluginInfo>;
  /** Deaktiviert ein aktives Plugin (Status `disabled`); sein Ort ist danach `disconnected`. */
  pluginDeactivate(id: string): Promise<PluginInfo>;
  /** Entfernt die Bestätigung (Pin) und alle Rechte: das Plugin wird deaktiviert, der Status danach ist `new`. */
  pluginTrustReset(id: string): Promise<void>;
  /** Bereinigter Diagnosetext (höchstens 512 Zeichen, nur Text). */
  pluginDiagnostics(id: string): Promise<string>;
  /**
   * Werte von Plugin-Spalten für Pfade (höchstens `MAX_PLUGIN_COLUMN_PATHS` Pfade und
   * `MAX_PLUGIN_COLUMN_IDS` Spalten, sonst `invalidInput`). Nur Spalten aktiver Plugins, sonst
   * `notFound`. Ergebnis je angefragter Spalte in gleicher Reihenfolge; `values[i]` gehört zu `paths[i]`.
   */
  pluginColumns(paths: string[], columnIds: string[]): Promise<PluginColumnValues[]>;
  /**
   * Vorschau einer Datei durch ein Vorschau-Plugin (`pluginId`, sonst das erste aktive). Nur Text oder
   * Rasterbild, nie HTML/SVG; Nicht-Darstellbares: `unsupported`. Ist das genannte Plugin nicht
   * aktiv: `pluginDisabled`. Weitere Fehlerarten: `pluginLocked`, `pluginSandboxUnavailable`,
   * `pluginTimeout`, `pluginProtocol`, `pluginCrashed`, `pluginTooLarge`, `pluginNotInstalled`,
   * `pluginUnsafe`, `pluginBusy`, `pluginFailed`.
   */
  pluginPreview(path: string, pluginId?: string): Promise<PluginPreview>;
  /** Liefert die Abmeldefunktion. */
  onEvent(handler: (event: CoreEvent) => void): () => void;
}
