import { undoDescription } from "../state/undoPresentation";
import { useEffect, useRef, useState } from "react";
import { useAppState, useController, useT } from "../app/context";
import type { UndoEntry } from "../ipc/types";

export function UndoToast({ entry }: { entry: UndoEntry }) {
  const controller = useController();
  const tr = useT();
  const modal = useAppState((state) => !!(state.dialog || state.conflicts.length || state.lister || state.compare || state.diff));
  const busy = useAppState((state) => state.undoBusy);
  const undone = useAppState((state) => state.history.find((item) => item.id === entry.id)?.undone ?? true);
  const [hover, setHover] = useState(false);
  const [focus, setFocus] = useState(false);
  const remaining = useRef(6000);
  const label = undoDescription(entry, tr);
  useEffect(() => {
    if (hover || focus || modal) return;
    const start = Date.now();
    const timer = setTimeout(controller.dismissUndoToast, remaining.current);
    return () => { clearTimeout(timer); remaining.current = Math.max(0, remaining.current - (Date.now() - start)); };
  }, [controller, hover, focus, modal]);
  return <div className="toast undo-toast" role="status"
    onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
    onFocus={() => setFocus(true)} onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) setFocus(false); }}>
    <span>{label}</span>
    <button className="tb" aria-label={tr("undo.button", { label })} disabled={busy || undone} onClick={() => void controller.undo(entry.id)}>{tr("action.undo")}</button>
  </div>;
}
