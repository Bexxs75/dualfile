import { useState, type FormEvent } from "react";
import { useController, useT } from "../../app/context";
import type { PackFormat } from "../../ipc/types";
import { PACK_FORMATS } from "../../ipc/types";
import { nameError } from "../../state/names";
import { PACK_EXTENSIONS, withPackExtension } from "../../state/pack";
import { displayPath } from "../../state/paths";
import { Modal } from "../Modal";

const FORMAT_LABEL: Record<PackFormat, string> = { zip: "ZIP", tarGz: "TAR.GZ", tarBz2: "TAR.BZ2", tarXz: "TAR.XZ", sevenZ: "7Z" };

interface PackDialogProps {
  sourceNames: readonly string[];
  destination: string;
  existing: readonly string[];
  suggestion: string;
}

/**
 * Strg+Umschalt+P: Format, Name (Vorschlag aus dem ersten Namen plus Endung) und Ziel (Ordner des
 * Gegenfensters, fest). Ein vorhandener Name wird nie überschrieben: der Fehler steht am Feld, der Name ist änderbar.
 * Fortschritt und Abbruch zeigt danach das Auftragspanel.
 */
export function PackDialog({ sourceNames, destination, existing, suggestion }: PackDialogProps) {
  const controller = useController();
  const tr = useT();
  const [format, setFormat] = useState<PackFormat>("zip");
  const [name, setName] = useState(withPackExtension(suggestion, "zip"));
  const [serverError, setServerError] = useState<string | null>(null);
  const check = nameError(name, existing);
  const shown = check ? tr(check) : serverError;
  const preview = sourceNames.slice(0, 5);

  function changeFormat(next: PackFormat): void {
    setFormat(next);
    // Die Endung folgt dem Format; ein selbst getippter Name bleibt ansonsten unangetastet.
    setName((current) => withPackExtension(current, next));
    setServerError(null);
  }

  async function onSubmit(event: FormEvent): Promise<void> {
    event.preventDefault();
    if (check) return;
    setServerError(await controller.submitPack(format, name));
  }

  return (
    <Modal
      title={tr("dialog.pack.title")}
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button type="button" className="tb" onClick={() => controller.closeDialog()}>
            {tr("action.cancel")}
          </button>
          <button type="submit" form="pack-form" className="tb primary" disabled={check !== null}>
            {tr("dialog.pack.confirm")}
          </button>
        </>
      }
    >
      <form id="pack-form" className="form" onSubmit={(event) => void onSubmit(event)}>
        <p>{tr("dialog.pack.summary", { count: sourceNames.length })}</p>
        <ul className="names">
          {preview.map((entry) => (
            <li key={entry}>{entry}</li>
          ))}
          {sourceNames.length > preview.length ? <li>{tr("dialog.delete.more", { count: sourceNames.length - preview.length })}</li> : null}
        </ul>
        <label className="fld">
          <span>{tr("dialog.pack.format")}</span>
          <select value={format} onChange={(event) => changeFormat(event.target.value as PackFormat)}>
            {PACK_FORMATS.map((entry) => (
              <option key={entry} value={entry}>
                {FORMAT_LABEL[entry]} ({PACK_EXTENSIONS[entry]})
              </option>
            ))}
          </select>
        </label>
        <label className="fld">
          <span>{tr("dialog.pack.name")}</span>
          <input
            data-autofocus
            value={name}
            spellCheck={false}
            aria-invalid={shown !== null}
            aria-describedby={shown ? "pack-error" : undefined}
            onChange={(event) => {
              setName(event.target.value);
              setServerError(null);
            }}
          />
        </label>
        <p className="muted">{tr("dialog.pack.destination", { path: displayPath(destination) })}</p>
        {shown ? (
          <p id="pack-error" className="field-error" role="alert">
            {shown}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}
