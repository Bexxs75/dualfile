import { useState, type FormEvent } from "react";
import { useController, useT } from "../../app/context";
import type { WinIdx } from "../../app/state";
import { Modal } from "../Modal";

/** `+` und `-`: Auswahl nach Muster (mehrere mit `;`, `*` und `?`). */
export function PatternDialog({ mode, w }: { mode: "select" | "deselect"; w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const [pattern, setPattern] = useState("*");
  const title = tr(mode === "select" ? "menu.edit.selectPattern" : "menu.edit.deselectPattern");
  function submit(event: FormEvent): void {
    event.preventDefault();
    controller.applyPattern(mode, w, pattern);
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
          <button type="submit" form="pattern-form" className="tb primary">
            {tr("action.ok")}
          </button>
        </>
      }
    >
      <form id="pattern-form" className="form" onSubmit={submit}>
        <label className="fld">
          <span>{title}</span>
          <input data-autofocus value={pattern} spellCheck={false} onChange={(event) => setPattern(event.target.value)} onFocus={(event) => event.currentTarget.select()} />
        </label>
      </form>
    </Modal>
  );
}
