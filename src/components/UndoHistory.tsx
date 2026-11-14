import { undoDescription, undoShortcuts } from "../state/undoPresentation";
import { useRef } from "react";
import { useAppState, useController, useT } from "../app/context";
import { Modal } from "./Modal";
import { useDialogFocus } from "./dialogs/useDialogFocus";

export function UndoHistory() {
  const tr = useT();
  const controller = useController();
  const history = useAppState((state) => state.history);
  const busy = useAppState((state) => state.undoBusy);
  const job = useAppState((state) => state.undoJob);
  const results = useAppState((state) => state.undoResults);
  const anchor = useRef<HTMLDivElement>(null);
  useDialogFocus(anchor, `${busy}:${history.map((entry) => entry.undone).join()}`);
  return <Modal title={tr("undo.history")} onClose={controller.closeDialog} wide>
    <div ref={anchor}>
      <p className="hint">{tr("undo.help", undoShortcuts(navigator.platform))}</p>
      {history.length === 0 ? <p>{tr("undo.empty")}</p> : <ul className="undo-history" role="list">
        {history.map((entry) => {
          const label = undoDescription(entry, tr);
          const result = results[entry.id];
          const date = new Date(entry.time);
          const time = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
          return <li key={entry.id} className={entry.undone ? "undone" : ""}>
            <div className="undo-row">
              <time dateTime={date.toISOString()}>{time}</time>
              <span className="undo-label">{label}</span>
              <button className="tb" disabled={entry.undone || busy} aria-label={tr("undo.button", { label })} onClick={() => void controller.undo(entry.id)}>{tr("action.undo")}</button>
            </div>
            {result ? <div role="status">
              {tr("undo.summary", { done: result.done, total: result.done + result.remaining, remaining: result.remaining })}
              {result.cancelled ? <p>{tr("undo.cancelled")}</p> : null}
              {[...new Set(result.errors)].map((error) => <p key={error}>{tr(`undoError.${error}`)}</p>)}
            </div> : null}
          </li>;
        })}
      </ul>}
      {busy && job !== null ? <button className="tb" onClick={() => void controller.cancelJob(job)}>{tr("jobs.cancel")}</button> : null}
    </div>
  </Modal>;
}
