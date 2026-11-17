import { useController, useT } from "../../app/context";
import { Modal } from "../Modal";

/** Confirmation before items are deleted from the trash for good; focus starts on "Cancel". */
export function TrashPurgeDialog({ names }: { names: readonly string[] }) {
  const controller = useController();
  const tr = useT();
  return (
    <Modal
      role="alertdialog"
      title={tr("trash.purgeTitle", { count: names.length, name: names[0] ?? "" })}
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button type="button" className="tb" data-autofocus onClick={() => controller.closeDialog()}>{tr("action.cancel")}</button>
          <button type="button" className="tb danger" onClick={() => void controller.confirmTrashPurge()}>{tr("trash.purgeConfirm")}</button>
        </>
      }
    >
      <p>{tr("trash.purgeBody")}</p>
      <ul>
        {names.slice(0, 4).map((name, index) => <li key={index}>{name}</li>)}
        {names.length > 4 ? <li>{tr("trash.more", { count: names.length - 4 })}</li> : null}
      </ul>
    </Modal>
  );
}
