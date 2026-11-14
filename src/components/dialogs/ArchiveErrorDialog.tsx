import { useController, useT } from "../../app/context";
import { baseName } from "../../state/paths";
import { Modal } from "../Modal";

/** Ein Archiv ließ sich nicht öffnen: Das Fenster ist unverändert im Ordner geblieben; wahlweise extern öffnen. */
export function ArchiveErrorDialog({ path, message, external }: { path: string; message: string; external: boolean }) {
  const controller = useController();
  const tr = useT();
  return (
    <Modal
      title={tr("dialog.archiveError.title", { name: baseName(path) })}
      role="alertdialog"
      onClose={() => controller.closeDialog()}
      footer={
        <>
          {external ? (
            <button
              type="button"
              className="tb"
              onClick={() => {
                controller.closeDialog();
                void controller.openExternalPath(path);
              }}
            >
              {tr("dialog.archiveError.external")}
            </button>
          ) : null}
          <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeDialog()}>
            {tr("dialog.archiveError.leave")}
          </button>
        </>
      }
    >
      <p role="alert">{message}</p>
    </Modal>
  );
}
