import { useController, useLanguage, useT } from "../../app/context";
import type { Entry } from "../../ipc/types";
import { createTranslator, formatBytes, formatDate } from "../../i18n";
import { fileType } from "../../state/sort";
import { Modal } from "../Modal";

export function PropertiesDialog({ entry, path }: { entry: Entry; path: string }) {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const kind =
    entry.kind === "dir"
      ? tr("type.folder")
      : entry.kind === "symlink"
        ? tr(entry.brokenLink ? "type.brokenLink" : "type.link")
        : fileType(entry.name) === ""
          ? tr("type.file")
          : createTranslator(language)("type.fileWithExtension", { extension: fileType(entry.name).toUpperCase() });
  return (
    <Modal
      title={tr("dialog.properties.title")}
      onClose={() => controller.closeDialog()}
      footer={
        <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeDialog()}>
          {tr("action.close")}
        </button>
      }
    >
      <dl className="props">
        <dt>{tr("dialog.properties.name")}</dt>
        <dd>{entry.name}</dd>
        <dt>{tr("dialog.properties.kind")}</dt>
        <dd>{kind}</dd>
        <dt>{tr("dialog.properties.path")}</dt>
        <dd>
          <code>{path}</code>
        </dd>
        {entry.kind === "file" ? (
          <>
            <dt>{tr("dialog.properties.size")}</dt>
            <dd>{formatBytes(entry.size, language)}</dd>
          </>
        ) : null}
        <dt>{tr("dialog.properties.modified")}</dt>
        <dd>{formatDate(entry.modified, language)}</dd>
        {entry.mode !== null ? (
          <>
            <dt>{tr("dialog.properties.permissions")}</dt>
            <dd>
              <code>{(entry.mode & 0o7777).toString(8).padStart(3, "0")}</code>
            </dd>
          </>
        ) : null}
        {entry.linkTarget ? (
          <>
            <dt>{tr("dialog.properties.linkTarget")}</dt>
            <dd>
              <code>{entry.linkTarget}</code>
            </dd>
          </>
        ) : null}
      </dl>
    </Modal>
  );
}
