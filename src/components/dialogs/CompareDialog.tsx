import { useRef } from "react";
import { useController, useLanguage, useT } from "../../app/context";
import { formatBytes, formatDate, type TranslationKey } from "../../i18n";
import type { CompareSide, CompareState } from "../../ipc/types";
import { COMPARE_STATES } from "../../ipc/types";
import { allowedSyncModes, canDiffEntry, COMPARE_PAGE_SIZE, STATE_MARKS, type CompareSession } from "../../state/compare";
import { displayPath, joinPath } from "../../state/paths";
import { Modal } from "../Modal";
import { useDialogFocus } from "./useDialogFocus";

/** Hinweis: der Kern liefert `states` fest; die Beschriftung steht je Zustand im Wörterbuch. */
const stateLabel = (state: CompareState): TranslationKey => `compare.state.${state}`;

function SideCell({ side }: { side: CompareSide | null }) {
  const tr = useT();
  const language = useLanguage();
  if (!side) return <span className="muted">–</span>;
  if (side.kind === "dir") return <span>{tr("compare.kind.dir")}</span>;
  if (side.kind === "symlink") return <span>{tr("compare.kind.symlink", { target: side.linkTarget ?? "" })}</span>;
  return (
    <span>
      {formatBytes(side.size, language)}
      <span className="muted"> · {formatDate(side.modified, language)}</span>
    </span>
  );
}

/**
 * Strg+Umschalt+D: vergleicht die Ordner beider Fenster. Optionen, Auftrag mit Fortschritt und Abbruch,
 * danach die Ergebnisliste seitenweise mit Filter, Zeilenauswahl und Einstieg in Abgleich und Textvergleich.
 * Das Ergebnis wird beim Schließen freigegeben.
 */
export function CompareDialog({ session }: { session: CompareSession }) {
  const controller = useController();
  const tr = useT();
  const anchor = useRef<HTMLDivElement>(null);
  const running = session.phase === "running";
  useDialogFocus(anchor, session.phase);
  const page = session.page;
  const summary = page?.summary;
  const modes = allowedSyncModes(session.left, session.right);
  const ready = session.phase === "ready" && page !== null;
  const from = page && page.total > 0 ? page.offset + 1 : 0;
  const to = page ? page.offset + page.entries.length : 0;
  const pageRels = page?.entries.map((entry) => entry.rel) ?? [];

  return (
    <Modal
      title={tr("compare.title")}
      className="cmp"
      onClose={() => controller.closeCompare()}
      footer={
        <>
          <button
            type="button"
            className="tb"
            disabled={!ready || modes.length === 0}
            title={modes.length === 0 ? tr("compare.syncHint") : undefined}
            onClick={() => controller.openSync()}
          >
            {tr("compare.sync")}
          </button>
          {modes.length === 0 ? <span className="hint">{tr("compare.syncHint")}</span> : null}
          <span className="sp" />
          <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeCompare()}>
            {tr("action.close")}
          </button>
        </>
      }
    >
      <div ref={anchor} className="cmp-body">
        <div className="cmp-roots">
          <span>{tr("compare.left", { path: displayPath(session.left) })}</span>
          <span>{tr("compare.right", { path: displayPath(session.right) })}</span>
        </div>
        <fieldset className="cmp-opts" disabled={running}>
          <legend>{tr("compare.options")}</legend>
          <label className="check">
            <input
              type="checkbox"
              checked={session.options.mode === "content"}
              onChange={(event) => controller.setCompareOptions({ mode: event.target.checked ? "content" : "sizeTime" })}
            />
            {tr("compare.option.content")}
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={session.options.recursive}
              onChange={(event) => controller.setCompareOptions({ recursive: event.target.checked })}
            />
            {tr("compare.option.recursive")}
          </label>
          <label className="check">
            <input type="checkbox" checked={session.includeHidden} onChange={event => controller.setCompareHidden(event.target.checked)} />
            {tr("compare.option.hidden")}
          </label>
          <button type="button" className="tb" onClick={() => void controller.startCompare()}>
            {session.phase === "setup" ? tr("compare.start") : tr("compare.again")}
          </button>
        </fieldset>

        {running ? (
          <div className="cmp-run" role="status">
            <progress aria-label={tr("compare.title")} />
            <span>{tr("compare.running", { count: session.status?.filesDone ?? 0 })}</span>
            <button type="button" className="tb small" onClick={() => void controller.cancelCompare()}>
              {tr("compare.cancel")}
            </button>
          </div>
        ) : null}
        {session.phase === "cancelled" ? <p role="status">{tr("compare.cancelled")}</p> : null}
        {session.phase === "failed" ? (
          <p className="field-error" role="alert">
            {tr("compare.failed", { reason: session.error ?? "" })}
          </p>
        ) : null}

        {ready && summary ? (
          <>
            {summary.incomplete ? (
              <div className="cmp-incomplete" role="status">
                <strong>{tr("compare.incomplete")}</strong>
                {summary.issues.length > 0 ? (
                  <details>
                    <summary>{tr("compare.issues", { count: summary.issues.length })}</summary>
                    <ul>
                      {summary.issues.slice(0, 20).map((issue, index) => (
                        <li key={index}>{issue}</li>
                      ))}
                    </ul>
                  </details>
                ) : null}
              </div>
            ) : null}
            <div className="cmp-filter">
              <div role="group" aria-label={tr("compare.filter.states")} className="cmp-states">
                {COMPARE_STATES.map((state) => {
                  const on = session.filter.states.includes(state);
                  return (
                    <button
                      key={state}
                      type="button"
                      className={on ? `tb small on tone-${STATE_MARKS[state].tone}` : `tb small tone-${STATE_MARKS[state].tone}`}
                      aria-pressed={on}
                      onClick={() => controller.toggleCompareState(state)}
                    >
                      <span aria-hidden="true">{STATE_MARKS[state].symbol}</span> {tr(stateLabel(state))} ({summary.counts[state]})
                    </button>
                  );
                })}
              </div>
              <label className="fld cmp-text">
                <span>{tr("compare.filter.text")}</span>
                <input
                  type="search"
                  value={session.filter.text}
                  spellCheck={false}
                  onChange={(event) => controller.setCompareFilter({ text: event.target.value })}
                />
              </label>
            </div>
            <p className="muted" role="status">
              {tr("compare.summary", { entries: summary.entries, total: page.total })}
            </p>
            <div className="cmp-scroll">
              <table className="tbl cmp-table" aria-label={tr("compare.table")}>
                <thead>
                  <tr>
                    <th scope="col">{tr("compare.col.select")}</th>
                    <th scope="col">{tr("compare.col.path")}</th>
                    <th scope="col">{tr("compare.col.left")}</th>
                    <th scope="col">{tr("compare.col.state")}</th>
                    <th scope="col">{tr("compare.col.right")}</th>
                    <th scope="col">{tr("compare.col.action")}</th>
                  </tr>
                </thead>
                <tbody>
                  {page.entries.map((entry) => {
                    const mark = STATE_MARKS[entry.state];
                    const path = entry.rel === "" ? tr("sync.notes.root") : entry.rel;
                    const chosen = session.selected.has(entry.rel);
                    return (
                      <tr key={entry.rel} className={`tone-${mark.tone}${chosen ? " chosen" : ""}`} data-state={entry.state}>
                        <td>
                          <input
                            type="checkbox"
                            checked={chosen}
                            aria-label={tr("compare.row.select", { path })}
                            onChange={(event) => controller.setCompareRows([entry.rel], event.target.checked)}
                          />
                        </td>
                        <td className="rel">
                          <code>{path}</code>
                        </td>
                        <td>
                          <SideCell side={entry.left} />
                        </td>
                        <td className="st">
                          <span className="mark" aria-hidden="true">
                            {mark.symbol}
                          </span>{" "}
                          {tr(stateLabel(entry.state))}
                          {entry.state === "unknown" && entry.note ? (
                            <div className="muted">{tr("compare.unknownNote", { reason: entry.note })}</div>
                          ) : null}
                        </td>
                        <td>
                          <SideCell side={entry.right} />
                        </td>
                        <td>
                          {canDiffEntry(entry) ? (
                            <button
                              type="button"
                              className="tb small"
                              aria-label={tr("compare.row.diffLabel", { path })}
                              onClick={() => controller.openDiff(joinPath(session.left, entry.rel), joinPath(session.right, entry.rel))}
                            >
                              {tr("compare.row.diff")}
                            </button>
                          ) : null}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
              {page.entries.length === 0 ? <p className="pane-msg">{tr("compare.empty")}</p> : null}
            </div>
            <div className="cmp-pager">
              <button type="button" className="tb small" disabled={page.offset === 0} onClick={() => void controller.compareGoto(page.offset - COMPARE_PAGE_SIZE)}>
                {tr("compare.page.prev")}
              </button>
              <span role="status">{tr("compare.page", { from, to, total: page.total })}</span>
              <button type="button" className="tb small" disabled={to >= page.total} onClick={() => void controller.compareGoto(to)}>
                {tr("compare.page.next")}
              </button>
              <span className="sp" />
              <span>{tr("compare.selected", { count: session.selected.size })}</span>
              <button type="button" className="tb small" onClick={() => controller.setCompareRows(pageRels, true)}>
                {tr("compare.selectPage")}
              </button>
              <button type="button" className="tb small" disabled={session.selected.size === 0} onClick={() => controller.clearCompareRows()}>
                {tr("compare.clearSelection")}
              </button>
            </div>
          </>
        ) : null}
      </div>
    </Modal>
  );
}
