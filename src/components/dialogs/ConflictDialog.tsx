import { useState } from "react";
import { useAppState, useController, useLanguage, useT } from "../../app/context";
import type { ConflictInfo } from "../../ipc/types";
import { formatBytes, formatDate } from "../../i18n";
import { conflictDisplayPath } from "../../state/paths";
import { Modal } from "../Modal";

/**
 * Konflikt beim Ziel. Der Standardknopf ist „Überspringen“ – Überschreiben gibt es nur durch
 * ausdrücklichen Klick; Esc bricht den Auftrag ab (es wird nichts überschrieben).
 */
export function ConflictDialog({ conflict }: { conflict: ConflictInfo }) {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const destination = useAppState(state => state.jobDestinations[conflict.jobId]);
  const [all, setAll] = useState(false);
  const answer = (resolution: "overwrite" | "skip" | "keepBoth" | "cancel") => () =>
    void controller.answerConflict(conflict, resolution, all);
  const describe = (entry: ConflictInfo["source"]) => ({
    size: formatBytes(entry.size, language),
    date: formatDate(entry.modified, language),
  });
  return (
    <Modal
      key={`${conflict.jobId}-${conflict.destinationPath}`}
      role="alertdialog"
      title={tr("dialog.conflict.title")}
      onClose={answer("cancel")}
      footer={
        <>
          <button type="button" className="tb" onClick={answer("overwrite")}>
            {tr("dialog.conflict.overwrite")}
          </button>
          <button type="button" className="tb primary" data-autofocus onClick={answer("skip")}>
            {tr("dialog.conflict.skip")}
          </button>
          <button type="button" className="tb" onClick={answer("keepBoth")}>
            {tr("dialog.conflict.keepBoth")}
          </button>
          <button type="button" className="tb" onClick={answer("cancel")}>
            {tr("dialog.conflict.cancel")}
          </button>
        </>
      }
    >
      <p>{tr("dialog.conflict.body", { name: conflictDisplayPath(conflict.destinationPath, destination) })}</p>
      <p className="muted">{tr("dialog.conflict.source", describe(conflict.source))}</p>
      <p className="muted">{tr("dialog.conflict.destination", describe(conflict.destination))}</p>
      <label className="check">
        <input type="checkbox" checked={all} onChange={(event) => setAll(event.target.checked)} />
        {tr("dialog.conflict.applyToAll")}
      </label>
    </Modal>
  );
}
