import { useState, type FormEvent } from "react";
import { useController, useT } from "../../app/context";
import { nameError } from "../../state/names";
import { Modal } from "../Modal";

interface NameDialogProps {
  title: string;
  label: string;
  initial: string;
  existing: readonly string[];
  /** Bisheriger Name beim Umbenennen. */
  current?: string;
  submit: (name: string) => Promise<string | null>;
}

/** Gemeinsame Grundlage von Umbenennen (F2) und Neuer Ordner (F7): Namensprüfung mit Fehler direkt am Feld. */
export function NameDialog({ title, label, initial, existing, current, submit }: NameDialogProps) {
  const controller = useController();
  const tr = useT();
  const [value, setValue] = useState(initial);
  const [touched, setTouched] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const check = nameError(value, existing, current);
  const shown = (touched || value !== initial) && check ? tr(check) : serverError;

  async function onSubmit(event: FormEvent): Promise<void> {
    event.preventDefault();
    setTouched(true);
    if (check) return;
    setServerError(await submit(value));
  }

  return (
    <Modal
      title={title}
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button type="button" className="tb" onClick={() => controller.closeDialog()}>
            {tr("action.cancel")}
          </button>
          <button type="submit" form="name-form" className="tb primary">
            {tr("action.ok")}
          </button>
        </>
      }
    >
      <form id="name-form" className="form" onSubmit={(event) => void onSubmit(event)}>
        <label className="fld">
          <span>{label}</span>
          <input
            data-autofocus
            value={value}
            spellCheck={false}
            aria-invalid={shown !== null}
            aria-describedby={shown ? "name-error" : undefined}
            onChange={(event) => {
              setValue(event.target.value);
              setServerError(null);
            }}
            onFocus={(event) => {
              const dot = initial.lastIndexOf(".");
              if (current !== undefined) event.currentTarget.setSelectionRange(0, dot > 0 ? dot : initial.length);
            }}
          />
        </label>
        {shown ? (
          <p id="name-error" className="field-error" role="alert">
            {shown}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}

export function RenameDialog({ name, existing }: { name: string; existing: readonly string[] }) {
  const controller = useController();
  const tr = useT();
  return (
    <NameDialog
      title={tr("dialog.rename.title")}
      label={tr("dialog.rename.label", { name })}
      initial={name}
      current={name}
      existing={existing}
      submit={(value) => controller.submitRename(value)}
    />
  );
}

export function MakeDirDialog({ existing }: { existing: readonly string[] }) {
  const controller = useController();
  const tr = useT();
  return (
    <NameDialog
      title={tr("dialog.newFolder.title")}
      label={tr("dialog.newFolder.label")}
      initial=""
      existing={existing}
      submit={(value) => controller.submitMakeDir(value)}
    />
  );
}
