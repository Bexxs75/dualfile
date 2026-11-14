import type { CoreEvent, DeletePlan, Entry, JobState, JobStatus } from "../ipc/types";
import type { TranslationKey, TranslationParams } from "../i18n";
import { isArchivePath, joinPath, normalizePath } from "./paths";

/** Das, was ein Fenster für eine Operation preisgibt (kein Zugriff auf den Fensterzustand nötig). */
export interface PaneSnapshot {
  path: string;
  entries: readonly Entry[];
  selected: ReadonlySet<string>;
  focus: number;
}

export type OperationKind = "copy" | "move";

/** Beschreibung für den Bestätigungsdialog von F5/F6. */
export interface OperationPlan {
  kind: OperationKind;
  /** Absolute Quellpfade, in Listenreihenfolge. */
  sources: string[];
  sourceNames: string[];
  /** Immer vorbelegt mit dem Pfad des anderen Fensters; im Dialog änderbar (`withDestination`). */
  destination: string;
  count: number;
  fileCount: number;
  folderCount: number;
  /** Summe der Dateigrößen. Ordnerinhalt ist ohne Durchlauf unbekannt, siehe `bytesIncomplete`. */
  bytes: number;
  bytesIncomplete: boolean;
  /** Ziel = Quellordner: der Dialog muss das melden, der Kern würde sonst Konflikte mit sich selbst melden. */
  sameFolder: boolean;
  /** Quelle liegt in einem Archiv: Kopieren heißt dann „Entpacken“. */
  extract: boolean;
}

/**
 * Wählt die Quellen: Markierung, sonst der Fokuseintrag. ".." wird nie genommen; steht der Fokus
 * auf ".." und nichts ist markiert, gibt es nichts zu tun (null). Das Ziel ist per Konstruktion der
 * Pfad des *anderen* Fensters – der Aufrufer übergibt hier `sourcePane` und `targetPane`.
 */
export function planOperation(kind: OperationKind, sourcePane: PaneSnapshot, targetPane: PaneSnapshot): OperationPlan | null {
  const marked = sourcePane.entries.filter((entry) => entry.name !== ".." && sourcePane.selected.has(entry.name));
  const focused = sourcePane.entries[sourcePane.focus];
  const chosen = sourcePane.selected.size > 0 ? marked : focused && focused.name !== ".." ? [focused] : [];
  if (chosen.length === 0) return null;
  const files = chosen.filter((entry) => entry.kind !== "dir");
  const folders = chosen.length - files.length;
  return {
    kind,
    sources: chosen.map((entry) => joinPath(sourcePane.path, entry.name)),
    sourceNames: chosen.map((entry) => entry.name),
    destination: normalizePath(targetPane.path),
    count: chosen.length,
    fileCount: files.length,
    folderCount: folders,
    bytes: files.reduce((sum, entry) => sum + entry.size, 0),
    bytesIncomplete: folders > 0,
    sameFolder: normalizePath(sourcePane.path) === normalizePath(targetPane.path),
    extract: kind === "copy" && isArchivePath(sourcePane.path),
  };
}

export const planCopy = (source: PaneSnapshot, target: PaneSnapshot): OperationPlan | null =>
  planOperation("copy", source, target);
export const planMove = (source: PaneSnapshot, target: PaneSnapshot): OperationPlan | null =>
  planOperation("move", source, target);

/** Ziel im Dialog ändern. Leer wird abgelehnt (null); `sameFolder` zieht mit. */
export function withDestination(plan: OperationPlan, destination: string, sourceFolder: string): OperationPlan | null {
  if (destination.trim() === "") return null;
  const norm = normalizePath(destination.trim());
  return { ...plan, destination: norm, sameFolder: norm === normalizePath(sourceFolder) };
}

// --- Auftragsliste ---------------------------------------------------------------------------

export interface JobsState {
  /** In Startreihenfolge; fertige Aufträge bleiben stehen, bis sie geschlossen werden. */
  jobs: readonly JobStatus[];
  /** Geschlossene Aufträge: verspätete Ereignisse dürfen sie nicht wieder öffnen. */
  closed: readonly number[];
}

export const emptyJobs: JobsState = { jobs: [], closed: [] };

const FINISHED: readonly JobState[] = ["done", "failed", "cancelled"];
export const isFinished = (state: JobState): boolean => FINISHED.includes(state);

/**
 * Übernimmt `job`-Ereignisse. Zähler laufen nie rückwärts (Ereignisse können überholt eintreffen),
 * ein beendeter Auftrag bleibt beendet, und die Fehlerliste schrumpft nicht. Andere Ereignisse
 * gehören nicht in die Auftragsliste.
 */
export function jobsReducer(state: JobsState, event: CoreEvent): JobsState {
  if (event.type !== "job") return state;
  const incoming = event.status;
  if (state.closed.includes(incoming.id)) return state;
  const prior = state.jobs.find((job) => job.id === incoming.id);
  if (!prior) return { ...state, jobs: [...state.jobs, incoming] };
  const merged: JobStatus = {
    ...incoming,
    state: isFinished(prior.state) && !isFinished(incoming.state) ? prior.state : incoming.state,
    filesDone: Math.max(prior.filesDone, incoming.filesDone),
    bytesDone: Math.max(prior.bytesDone, incoming.bytesDone),
    bytesTotal: Math.max(prior.bytesTotal, incoming.bytesTotal),
    failed: incoming.failed.length >= prior.failed.length ? incoming.failed : prior.failed,
    skipped: incoming.skipped.length >= prior.skipped.length ? incoming.skipped : prior.skipped,
    warnings: incoming.warnings.length >= prior.warnings.length ? incoming.warnings : prior.warnings,
  };
  return { ...state, jobs: state.jobs.map((job) => (job.id === merged.id ? merged : job)) };
}

/** Schließen entfernt den Auftrag aus der Liste. Laufende Aufträge lassen sich nicht schließen (erst abbrechen). */
export function closeJob(state: JobsState, id: number): JobsState {
  const job = state.jobs.find((j) => j.id === id);
  if (!job || !isFinished(job.state)) return state;
  return { jobs: state.jobs.filter((j) => j.id !== id), closed: [...state.closed, id] };
}

export interface JobSummary {
  id: number;
  name: string;
  state: JobState;
  /** 0…1, bei unbekannter Gesamtgröße 0; erst bei "done" garantiert 1. */
  progress: number;
  /** Anzahl Fehler und Gesamtzahl der Einträge (erledigte + fehlgeschlagene), für „3 von 120 nicht kopiert“. */
  failedCount: number;
  totalCount: number;
  failures: { path: string; message: string }[];
  skippedCount: number;
}

/**
 * Rohdaten für die Anzeige. Der Text („3 von 120 nicht kopiert“) entsteht in der Oberfläche über
 * i18n; hier stehen nur die Zahlen und die Gründe aus `failed`. `filesDone` zählt im Kern nur
 * erfolgreiche Einträge, daher ist die Gesamtzahl erledigt + fehlgeschlagen.
 */
export function summarizeJob(job: JobStatus): JobSummary {
  const raw = job.bytesTotal > 0 ? job.bytesDone / job.bytesTotal : 0;
  const progress = job.state === "done" ? 1 : Math.min(1, Math.max(0, raw));
  return {
    id: job.id,
    name: job.name,
    state: job.state,
    progress,
    failedCount: job.failed.length,
    totalCount: job.filesDone + job.failed.length,
    failures: job.failed.map((f) => ({ path: f.path, message: f.message })),
    skippedCount: job.skipped.length,
  };
}

// --- Löschdialog -----------------------------------------------------------------------------

/** Schlüssel des Hinweistextes für i18n; `warning` ist die deutsche Standardfassung. */
export const DELETE_WARNING_KEY = "dialog.delete.warning";

export interface DeleteDialogModel {
  token: string;
  files: number;
  dirs: number;
  links: number;
  bytes: number;
  /** Anzahl aller Einträge (Dateien, Ordner, Links), für „… und n weitere“. */
  total: number;
  firstNames: string[];
  moreCount: number;
  /** Der Plan ist unvollständig (z. B. Zählung abgebrochen): Zahlen sind Untergrenzen. */
  incomplete: boolean;
  /** Standardknopf ist immer „Abbrechen“: Enter löscht nie. */
  defaultButton: "cancel";
  buttons: readonly ["cancel", "confirm"];
  warningKey: typeof DELETE_WARNING_KEY;
  warning: string;
}

export function deleteDialogModel(plan: DeletePlan): DeleteDialogModel {
  const total = plan.files + plan.dirs + plan.links;
  return {
    token: plan.token,
    files: plan.files,
    dirs: plan.dirs,
    links: plan.links,
    bytes: plan.bytes,
    total,
    firstNames: [...plan.firstNames],
    moreCount: Math.max(0, total - plan.firstNames.length),
    incomplete: plan.incomplete,
    defaultButton: "cancel",
    buttons: ["cancel", "confirm"],
    warningKey: DELETE_WARNING_KEY,
    warning: "Das endgültige Löschen kann nicht rückgängig gemacht werden.",
  };
}

/** Bestätigt wird nur mit dem Token genau dieses Plans; ein leeres oder fremdes Token gilt nicht. */
export function canConfirmDelete(model: DeleteDialogModel, token: string): boolean {
  return token !== "" && model.token !== "" && token === model.token;
}

/** Token für `deletePermanent`, falls die Bestätigung gültig ist, sonst null (es wird nichts gesendet). */
export function confirmDelete(model: DeleteDialogModel, token: string): string | null {
  return canConfirmDelete(model, token) ? model.token : null;
}

type Translate = (key: TranslationKey, params?: TranslationParams) => string;

const TITLE_KINDS = new Set([
  "copy", "move", "extract", "pack", "download", "upload", "trash", "delete", "rename", "search", "sync", "space", "similar", "dupes", "dupesTrash",
]);

/**
 * Auftragsname in der Oberfläche: aus dem sprachneutralen `title` des Kerns über i18n gebildet
 * („Copy: x (+2)“). Ohne `title` oder bei unbekannter Art gilt der Name des Kerns (Rückfall).
 */
export function jobDisplayName(job: Pick<JobStatus, "name" | "title">, tr: Translate): string {
  const title = job.title;
  if (!title || !TITLE_KINDS.has(title.kind)) return job.name;
  const base = tr(`jobs.name.${title.kind}` as TranslationKey, { first: title.first });
  return title.more > 0 ? tr("jobs.name.more", { name: base, more: title.more }) : base;
}

/** Grund eines fehlgeschlagenen Eintrags: bei fester Fehlerart aus i18n, sonst der Text des Kerns. */
export function failureReason(failure: { path: string; message: string; kind?: string }, tr: Translate): string {
  switch (failure.kind) {
    case "disconnected":
      return tr("itemError.disconnected");
    case "permission":
      return tr("itemError.permission");
    case "verifyFailed":
      return tr("itemError.verifyFailed");
    case "notFound":
      return tr("error.notFound", { path: failure.path });
    case "cancelled":
      return tr("error.cancelled");
    case "readOnly":
      return tr("error.readOnly");
    case "invalidTarget":
      return tr("error.invalidTarget");
    default:
      return failure.message;
  }
}
