import { useState, type FormEvent } from "react";
import { useController, useLanguage, useT } from "../../app/context";
import type { OperationPlan } from "../../state/operations";
import { formatBytes } from "../../i18n";
import { isArchivePath, parentPath } from "../../state/paths";
import { withDestination } from "../../state/operations";
import { Modal } from "../Modal";

/** F5/F6: Quelle, Ziel (vorbelegt mit dem anderen Fenster, änderbar), Anzahl und Größe; Standardknopf bestätigt. */
export function ConfirmTransferDialog({ plan }: { plan: OperationPlan }) {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const [destination, setDestination] = useState(plan.destination);
  const sourceFolder = parentPath(plan.sources[0] ?? "/");
  const current = withDestination(plan, destination, sourceFolder);
  // Ins Archiv zu schreiben geht nie (der Kern lehnt es ebenfalls ab).
  const intoArchive = isArchivePath(destination.trim());
  const blocked = current === null || current.sameFolder || intoArchive;
  const copy = plan.kind === "copy";
  const preview = plan.sourceNames.slice(0, 5);

  function submit(event: FormEvent): void {
    event.preventDefault();
    if (!blocked) void controller.confirmTransfer(plan, destination);
  }

  return (
    <Modal
      title={tr(plan.extract ? "dialog.extract.title" : copy ? "dialog.copy.title" : "dialog.move.title")}
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button type="button" className="tb" onClick={() => controller.closeDialog()}>
            {tr("action.cancel")}
          </button>
          <button type="submit" form="transfer-form" className="tb primary" disabled={blocked}>
            {tr(plan.extract ? "dialog.transfer.extractConfirm" : copy ? "dialog.transfer.copyConfirm" : "dialog.transfer.moveConfirm")}
          </button>
        </>
      }
    >
      <form id="transfer-form" onSubmit={submit} className="form">
        <p>{tr("dialog.transfer.summary", { count: plan.count, source: sourceFolder })}</p>
        <ul className="names">
          {preview.map((name) => (
            <li key={name}>{name}</li>
          ))}
          {plan.count > preview.length ? <li>{tr("dialog.delete.more", { count: plan.count - preview.length })}</li> : null}
        </ul>
        <p className="muted">
          {plan.bytesIncomplete
            ? tr("ui.dialog.transfer.sizeIncomplete", { size: formatBytes(plan.bytes, language) })
            : tr("dialog.transfer.size", { size: formatBytes(plan.bytes, language) })}
        </p>
        <label className="fld">
          <span>{tr("dialog.transfer.destination")}</span>
          <input data-autofocus onFocus={event => event.currentTarget.select()} value={destination} onChange={(event) => setDestination(event.target.value)} spellCheck={false} />
        </label>
        {intoArchive ? (
          <p className="field-error" role="alert">
            {tr("error.readOnly")}
          </p>
        ) : null}
        {current?.sameFolder ? (
          <p className="field-error" role="alert">
            {tr("ui.dialog.transfer.sameFolder")}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}
