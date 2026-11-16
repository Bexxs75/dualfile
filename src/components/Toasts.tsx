import { useAppState, useController, useT } from "../app/context";

import { UndoToast } from "./UndoToast";

export function Toasts() {
  const undoToast = useAppState((state) => state.undoToast);
  const toasts = useAppState((state) => state.toasts);
  const controller = useController();
  const tr = useT();
  return (
    <div className="toasts" role="region" aria-label={tr("ui.toasts")}>
      {undoToast ? <UndoToast key={undoToast.id} entry={undoToast} /> : null}
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={toast.kind === "error" ? "toast error" : "toast"}
          role={toast.kind === "error" ? "alert" : "status"}
          onClick={() => controller.removeToast(toast.id)}
        >
          {toast.text}
          {toast.kind === "error" ? (
            <button
              className="tb"
              onClick={(event) => {
                event.stopPropagation();
                controller.removeToast(toast.id);
                controller.openDialog({ kind: "log" });
              }}
            >
              {tr("log.details")}
            </button>
          ) : null}
        </div>
      ))}
    </div>
  );
}
