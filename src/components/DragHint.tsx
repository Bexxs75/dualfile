import { useAppState, useT } from "../app/context";

/** Hinweis am Mauszeiger beim Ziehen in das andere Fenster oder auf eine Ordnerzeile: was beim Loslassen passiert. */
export function DragHint() {
  const drag = useAppState((state) => state.drag);
  const target = useAppState((state) => {
    const drag = state.drag;
    if (!drag) return null;
    if (drag.folder) return drag.folder.path;
    return drag.over === null ? null : state.windows[drag.over].tabs[state.windows[drag.over].activeTab]?.path ?? null;
  });
  const tr = useT();
  if (!drag || target === null) return null;
  return (
    <div className="drag-hint" role="status" style={{ left: drag.x + 14, top: drag.y + 14 }}>
      <strong>{tr(drag.mode === "move" ? "ui.drag.move" : "ui.drag.copy", { target })}</strong>
      <span>{tr(drag.mode === "move" ? "ui.drag.releaseHint" : "ui.drag.shiftHint")}</span>
    </div>
  );
}
