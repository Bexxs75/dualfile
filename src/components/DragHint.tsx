import { useAppState, useT } from "../app/context";

/** Hinweis am Mauszeiger beim Ziehen zwischen den Fenstern: was beim Loslassen passiert. */
export function DragHint() {
  const drag = useAppState((state) => state.drag);
  const target = useAppState((state) => (state.drag?.over === null || state.drag === null ? null : state.windows[state.drag.over].tabs[state.windows[state.drag.over].activeTab]?.path ?? null));
  const tr = useT();
  if (!drag || drag.over === null || target === null) return null;
  return (
    <div className="drag-hint" role="status" style={{ left: drag.x + 14, top: drag.y + 14 }}>
      <strong>{tr(drag.mode === "move" ? "ui.drag.move" : "ui.drag.copy", { target })}</strong>
      <span>{tr(drag.mode === "move" ? "ui.drag.releaseHint" : "ui.drag.shiftHint")}</span>
    </div>
  );
}
