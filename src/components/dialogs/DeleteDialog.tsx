import { useController, useLanguage, useT } from "../../app/context";
import { formatBytes } from "../../i18n";
import type { DeleteDialogModel } from "../../state/operations";
import { Modal } from "../Modal";

/**
 * Endgültiges Löschen: Umfang, Warntext, Standardknopf „Abbrechen“. Bestätigt wird nur mit dem
 * Token genau dieses Plans (`controller.confirmDelete` prüft es noch einmal).
 */
export function DeleteDialog({ model, reason }: { model: DeleteDialogModel; reason: "permanent" | "trashFailed" }) {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const counts = [
    model.files > 0 ? tr("dialog.delete.files", { count: model.files }) : null,
    model.dirs > 0 ? tr("dialog.delete.dirs", { count: model.dirs }) : null,
    model.links > 0 ? tr("dialog.delete.links", { count: model.links }) : null,
  ].filter((part): part is string => part !== null);
  return (
    <Modal
      role="alertdialog"
      title={tr("dialog.delete.title")}
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeDialog()}>
            {tr("action.cancel")}
          </button>
          <button type="button" className="tb warn" onClick={() => void controller.confirmDelete(model.token)}>
            {tr("dialog.delete.confirm")}
          </button>
        </>
      }
    >
      {reason === "trashFailed" ? <p className="field-error">{tr("ui.dialog.delete.trashFailed")}</p> : null}
      <p>
        <strong>{counts.join(", ")}</strong>
        {model.incomplete ? ` – ${tr("dialog.delete.incomplete")}` : null}
      </p>
      <p className="muted">{tr("dialog.delete.total", { size: formatBytes(model.bytes, language) })}</p>
      <ul className="names">
        {model.firstNames.map((name) => (
          <li key={name}>{name}</li>
        ))}
        {model.moreCount > 0 ? <li>{tr("dialog.delete.more", { count: model.moreCount })}</li> : null}
      </ul>
      <p className="field-error">{tr(model.warningKey)}</p>
      <p className="muted">{tr("dialog.delete.partialWarning")}</p>
    </Modal>
  );
}
