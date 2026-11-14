import { useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { useAppState, useController, useT } from "../../app/context";
import { focusedEntry, focusedFilePath, type WinIdx } from "../../app/state";
import { Modal } from "../Modal";
import { FileViewer, defaultPrefs, type ViewPrefs } from "./FileViewer";

/**
 * Lister (F3): großes Fenster mit Fokusfalle (wie die Dialoge). Zeigt die Datei, auf der der Fokus
 * des Fensters `w` steht. Pfeil links/rechts = vorherige/nächste Datei der Liste, Esc/Q/F3 schließt
 * (F3 schließt nicht, solange ein Suchbegriff gesetzt ist: dann ist es „weitersuchen“).
 */
export function Lister({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const path = useAppState((state) => focusedFilePath(state, w));
  const name = useAppState((state) => focusedEntry(state, w)?.name ?? "");
  const [prefs, setPrefs] = useState<ViewPrefs>(defaultPrefs);
  const root = useRef<HTMLDivElement>(null);

  // Beim Dateiwechsel verschwindet das fokussierte Element der alten Ansicht: der Fokus bleibt im Lister,
  // damit Pfeiltasten auch während des Ladens ankommen.
  useLayoutEffect(() => {
    const active = document.activeElement;
    if (!active || active === document.body) root.current?.focus();
  }, [path]);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const target = event.target;
    if (target instanceof HTMLElement && (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) || target.isContentEditable)) return;
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      controller.listerStep(event.key === "ArrowRight" ? 1 : -1);
    } else if (event.key === "q" || event.key === "Q" || event.key === "F3") {
      event.preventDefault();
      controller.closeLister();
    }
  }

  return (
    <Modal
      title={tr("lister.title", { name })}
      full
      onClose={controller.closeLister}
      footer={
        <>
          <span className="hint">{tr("lister.hint")}</span>
          <button type="button" className="tb" onClick={controller.closeLister}>
            {tr("action.close")}
          </button>
        </>
      }
    >
      <div ref={root} className="lister" data-autofocus data-lister-root tabIndex={-1} onKeyDown={onKeyDown}>
        {path ? (
          <FileViewer key={path} path={path} name={name} prefs={prefs} onPrefs={setPrefs} focusOnMount />
        ) : (
          <p className="pane-msg">{tr("lister.noFile")}</p>
        )}
      </div>
    </Modal>
  );
}
