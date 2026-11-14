import { useRef } from "react";
import { useController, useLanguage, useT } from "../../app/context";
import { formatBytes, type TranslationKey } from "../../i18n";
import type { JobStatus, SyncActionKind, SyncMode } from "../../ipc/types";
import {
  allowedSyncModes,
  isDestructive,
  neededDirs,
  SYNC_LOAD_LIMIT,
  SYNC_VIEW_PAGE_SIZE,
  type CompareSession,
  type SyncSession,
} from "../../state/compare";
import { Modal } from "../Modal";
import { useDialogFocus } from "./useDialogFocus";

const MODES: readonly SyncMode[] = ["leftToRight", "rightToLeft", "mirror"];
const KIND_SYMBOL: Record<SyncActionKind, string> = { makeDir: "+", copy: "+", replace: "⚠", trash: "✕" };
const kindLabel = (kind: SyncActionKind): TranslationKey => `sync.kind.${kind}`;

function Report({ status }: { status: JobStatus }) {
  const tr = useT();
  const reasonOf = (path: string): string | null => status.warnings.find((warning) => warning.path === path)?.message ?? null;
  const skipped = status.skipped.map((path) => ({ path, reason: reasonOf(path) }));
  const extra = status.warnings.filter((warning) => !status.skipped.includes(warning.path));
  return (
    <section className="sync-report" aria-label={tr("sync.report.title")}>
      <p>
        <strong>
          {tr("sync.report.summary", { done: status.filesDone, skipped: status.skipped.length, failed: status.failed.length })}
        </strong>
      </p>
      {status.state === "cancelled" ? <p role="status">{tr("sync.report.cancelled")}</p> : null}
      {skipped.length + extra.length > 0 ? (
        <>
          <h3>{tr("sync.report.skipped")}</h3>
          <ul className="names">
            {skipped.map((item) => (
              <li key={`s-${item.path}`}>
                <code>{item.path}</code> – {tr("jobs.reason", { reason: item.reason ?? tr("sync.report.skippedNoReason") })}
              </li>
            ))}
            {extra.map((item, index) => (
              <li key={`w-${item.path}-${index}`}>
                <code>{item.path}</code> – {tr("jobs.reason", { reason: item.message })}
              </li>
            ))}
          </ul>
          <p className="muted">{tr("sync.report.staleHint")}</p>
        </>
      ) : null}
      {status.failed.length > 0 ? (
        <>
          <h3>{tr("sync.report.failed")}</h3>
          <ul className="names">
            {status.failed.map((item, index) => (
              <li key={`f-${item.path}-${index}`}>
                <code>{item.path}</code> – {tr("jobs.reason", { reason: item.message })}
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </section>
  );
}

function Preview({ session, sync }: { session: CompareSession; sync: SyncSession }) {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const plan = sync.effective;
  const needed = neededDirs(sync.actions, sync.unchecked);
  const total = sync.actions.length;
  const from = Math.min(sync.viewOffset, Math.max(0, total - 1));
  const visible = sync.actions.slice(from, from + SYNC_VIEW_PAGE_SIZE);
  const destructive = (plan?.replaces ?? 0) + (plan?.trashes ?? 0) > 0;
  const notes = sync.base?.notes ?? [];
  const incomplete = sync.base?.incomplete ?? session.page?.summary.incomplete ?? false;
  return (
    <>
      {sync.base ? (
        <p className="sync-counts" role="status">
          {plan
            ? tr("sync.counts", {
                copies: plan.copies,
                replaces: plan.replaces,
                trashes: plan.trashes,
                dirs: plan.makeDirs,
                size: formatBytes(plan.bytes, language),
              })
            : tr("sync.nothingSelected")}
        </p>
      ) : null}
      {sync.mode === "mirror" && incomplete ? (
        <p className="field-error" role="alert">
          {tr("sync.incomplete")}
        </p>
      ) : null}
      {notes.length > 0 ? (
        <details className="sync-notes">
          <summary>{tr("sync.notes", { count: sync.base?.notesTotal ?? notes.length })}</summary>
          <ul>
            {notes.map((note, index) => (
              <li key={`${note.rel}-${index}`}>
                <code>{note.rel === "" ? tr("sync.notes.root") : note.rel}</code> – {note.reason}
              </li>
            ))}
            {(sync.base?.notesTotal ?? 0) > notes.length ? <li>{tr("sync.notes.more", { count: (sync.base?.notesTotal ?? 0) - notes.length })}</li> : null}
          </ul>
        </details>
      ) : null}
      {destructive ? (
        <p className="sync-danger" role="alert">
          <span aria-hidden="true">⚠ </span>
          {tr("sync.warn.destructive")}
        </p>
      ) : null}
      {sync.actionsPartial ? <p className="muted">{tr("sync.partial", { count: SYNC_LOAD_LIMIT })}</p> : null}
      {total === 0 ? <p className="pane-msg">{tr("sync.nothingToDo")}</p> : null}
      {total > 0 ? (
        <>
          <div className="cmp-pager">
            <button type="button" className="tb small" disabled={sync.actionsPartial} onClick={() => controller.setSyncAll(true)}>
              {tr("sync.checkAll")}
            </button>
            <button type="button" className="tb small" disabled={sync.actionsPartial} onClick={() => controller.setSyncAll(false)}>
              {tr("sync.uncheckAll")}
            </button>
            <span className="sp" />
            <button type="button" className="tb small" disabled={from === 0} onClick={() => controller.setSyncViewOffset(from - SYNC_VIEW_PAGE_SIZE)}>
              {tr("compare.page.prev")}
            </button>
            <span role="status">{tr("compare.page", { from: from + 1, to: from + visible.length, total })}</span>
            <button
              type="button"
              className="tb small"
              disabled={from + visible.length >= total}
              onClick={() => controller.setSyncViewOffset(from + SYNC_VIEW_PAGE_SIZE)}
            >
              {tr("compare.page.next")}
            </button>
          </div>
          <div className="cmp-scroll">
            <table className="tbl sync-table" aria-label={tr("sync.table")}>
              <thead>
                <tr>
                  <th scope="col">{tr("sync.col.select")}</th>
                  <th scope="col">{tr("sync.col.action")}</th>
                  <th scope="col">{tr("sync.col.path")}</th>
                  <th scope="col">{tr("sync.col.size")}</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((action) => {
                  const required = action.kind === "makeDir" && needed.has(action.rel);
                  const checked = required || !sync.unchecked.has(action.rel);
                  return (
                    <tr key={`${action.kind}-${action.rel}`} className={isDestructive(action) ? `danger ${action.kind}` : action.kind} data-kind={action.kind}>
                      <td>
                        <input
                          type="checkbox"
                          checked={checked}
                          disabled={required || sync.actionsPartial}
                          aria-label={tr("sync.row.select", { path: action.rel })}
                          onChange={() => controller.toggleSyncAction(action.rel)}
                        />
                      </td>
                      <td className="act">
                        <span aria-hidden="true">{KIND_SYMBOL[action.kind]}</span> {tr(kindLabel(action.kind))}
                        {required ? <span className="muted"> ({tr("sync.needed")})</span> : null}
                      </td>
                      <td>
                        <code>{action.rel}</code>
                      </td>
                      <td className="num">{action.kind === "copy" || action.kind === "replace" ? formatBytes(action.size, language) : ""}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </>
      ) : null}
    </>
  );
}

/**
 * Abgleich aus dem Vergleich: Richtung und Optionen, Vorschau JEDER Aktion (Überschreiben und Papierkorb
 * rot und mit Text), Abwahl einzelner Aktionen, Bestätigung nur über den ausdrücklichen Knopf mit Zähler.
 * Der Standardknopf ist „Abbrechen“: Enter führt nie aus. Danach Fortschritt und Bericht.
 */
export function SyncDialog({ session, sync }: { session: CompareSession; sync: SyncSession }) {
  const controller = useController();
  const tr = useT();
  const anchor = useRef<HTMLDivElement>(null);
  useDialogFocus(anchor, sync.phase);
  const modes = allowedSyncModes(session.left, session.right);
  const busy = sync.phase === "running" || sync.phase === "finished";
  const plan = sync.effective;
  const parts = plan
    ? [
        plan.copies > 0 ? tr("sync.part.copy", { count: plan.copies }) : null,
        plan.replaces > 0 ? tr("sync.part.replace", { count: plan.replaces }) : null,
        plan.trashes > 0 ? tr("sync.part.trash", { count: plan.trashes }) : null,
        plan.makeDirs > 0 && plan.copies + plan.replaces + plan.trashes === 0 ? tr("sync.part.dirs", { count: plan.makeDirs }) : null,
      ].filter((part): part is string => part !== null)
    : [];
  const canConfirm = sync.phase === "preview" && plan !== null && plan.totalActions > 0;
  const harmful = (plan?.replaces ?? 0) + (plan?.trashes ?? 0) > 0;
  const status = sync.status;
  const leftButton =
    sync.phase === "running"
      ? { text: tr("sync.cancelRun"), run: () => void controller.cancelSync() }
      : sync.phase === "finished"
        ? { text: tr("action.close"), run: () => controller.closeSync() }
        : { text: tr("action.cancel"), run: () => controller.closeSync() };

  return (
    <Modal
      role="alertdialog"
      title={tr("sync.title")}
      className="cmp"
      onClose={() => controller.closeSync()}
      footer={
        <>
          <button type="button" className="tb primary" data-autofocus onClick={leftButton.run}>
            {leftButton.text}
          </button>
          {sync.phase === "finished" ? (
            <button type="button" className="tb" onClick={() => void controller.recompare()}>
              {tr("compare.again")}
            </button>
          ) : null}
          {sync.phase === "error" ? (
            <button type="button" className="tb" onClick={() => controller.setSyncMode(sync.mode)}>
              {tr("sync.replan")}
            </button>
          ) : null}
          {sync.phase === "preview" || sync.phase === "planning" ? (
            <button
              type="button"
              className={harmful ? "tb warn" : "tb"}
              disabled={!canConfirm}
              onClick={() => void controller.confirmSync()}
            >
              {parts.length > 0 ? tr("sync.confirm", { parts: parts.join(", ") }) : tr("sync.confirm", { parts: "0" })}
            </button>
          ) : null}
        </>
      }
    >
      <div ref={anchor} className="sync-body">
        {sync.phase !== "finished" ? (
          <>
            <fieldset className="sync-mode" disabled={busy}>
              <legend>{tr("sync.mode")}</legend>
              {MODES.map((mode) => (
                <label key={mode} className="check">
                  <input
                    type="radio"
                    name="sync-mode"
                    checked={sync.mode === mode}
                    disabled={!modes.includes(mode)}
                    onChange={() => controller.setSyncMode(mode)}
                  />
                  {tr(`sync.mode.${mode}`)}
                </label>
              ))}
            </fieldset>
              <label className="check">
                <input disabled={busy} type="checkbox" checked={sync.replaceNewer} onChange={(event) => controller.setSyncReplaceNewer(event.target.checked)} />
                {tr("sync.replaceNewer")}
              </label>
              <p className="muted">{tr(sync.mode === "mirror" ? (sync.replaceNewer ? "sync.mirrorReplace" : "sync.mirrorMissing") : sync.replaceNewer ? "sync.replaceHint" : "sync.onlyMissing")}</p>
          </>
        ) : null}
        {sync.phase === "planning" ? <p role="status">{tr("sync.planning")}</p> : null}
        {sync.error ? (
          <p className="field-error" role="alert">
            {sync.error}
          </p>
        ) : null}
        {sync.phase === "preview" ? <Preview session={session} sync={sync} /> : null}
        {sync.phase === "running" ? (
          <div className="cmp-run" role="status">
            <progress
              aria-label={tr("sync.running")}
              max={status && status.bytesTotal > 0 ? status.bytesTotal : undefined}
              value={status && status.bytesTotal > 0 ? status.bytesDone : undefined}
            />
            <span>{tr("sync.running")}</span>
          </div>
        ) : null}
        {sync.phase === "finished" && status ? <Report status={status} /> : null}
      </div>
    </Modal>
  );
}
