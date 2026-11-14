import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import { useT } from "../app/context";

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

interface ModalProps {
  title: string;
  onClose: () => void;
  closeDisabled?: boolean;
  children: ReactNode;
  footer?: ReactNode;
  /** `alertdialog` für Rückfragen, bei denen etwas verloren gehen kann. */
  role?: "dialog" | "alertdialog";
  wide?: boolean;
  /** Fast bildschirmfüllend (Lister). */
  full?: boolean;
  /** Zusätzliche Klasse für Dialoge mit eigenem Maß (Mehrfach-Umbenennen). */
  className?: string;
}

/** Offene Dialoge, der oberste zuletzt: nur er reagiert auf Esc am Dokument. */
const openModals: symbol[] = [];

/** Ist irgendein Dialog offen? Die globalen Tastenbefehle ruhen dann (auch für Dialoge außerhalb von `state.dialog`). */
export function isModalOpen(): boolean {
  return openModals.length > 0;
}

/**
 * Dialog mit Fokusfalle: Tab und Umschalt+Tab bleiben im Dialog, Esc schließt, der Fokus startet auf
 * dem Element mit `data-autofocus` (sonst dem ersten) und geht beim Schließen zurück.
 */
export function Modal({ title, onClose, closeDisabled = false, children, footer, role = "dialog", wide = false, full = false, className }: ModalProps) {
  const tr = useT();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const closeRef = useRef(onClose);
  closeRef.current = () => { if (!closeDisabled) onClose(); };

  // Esc schließt den obersten Dialog auch dann, wenn der Fokus gerade außerhalb liegt (etwa nach einem
  // Klick auf den Rückgängig-Knopf im Hinweis). Liegt der Fokus im Dialog, behandelt `onKeyDown` die
  // Taste und stoppt sie; hier kommt dann nichts mehr an.
  useEffect(() => {
    const token = Symbol("modal");
    openModals.push(token);
    const onDocumentKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape" || event.defaultPrevented || openModals[openModals.length - 1] !== token) return;
      event.preventDefault();
      closeRef.current();
    };
    document.addEventListener("keydown", onDocumentKeyDown);
    return () => {
      document.removeEventListener("keydown", onDocumentKeyDown);
      const index = openModals.indexOf(token);
      if (index >= 0) openModals.splice(index, 1);
    };
  }, []);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const root = ref.current;
    const first = root?.querySelector<HTMLElement>("[data-autofocus]") ?? root?.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? root)?.focus();
    return () => {
      if (previous && document.contains(previous)) previous.focus();
    };
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeRef.current();
      return;
    }
    if (event.key !== "Tab") return;
    const root = ref.current;
    if (!root) return;
    const items = [...root.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (items.length === 0) {
      event.preventDefault();
      return;
    }
    const firstItem = items[0]!;
    const lastItem = items[items.length - 1]!;
    const current = document.activeElement;
    if (event.shiftKey && (current === firstItem || !root.contains(current))) {
      event.preventDefault();
      lastItem.focus();
    } else if (!event.shiftKey && (current === lastItem || !root.contains(current))) {
      event.preventDefault();
      firstItem.focus();
    }
  }

  return (
    <div className="modal">
      <div
        ref={ref}
        className={[full ? "dlg full" : wide ? "dlg wide" : "dlg", className].filter(Boolean).join(" ")}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <header>
          <span id={titleId}>{title}</span>
          <button type="button" className="x" aria-label={tr("action.close")} disabled={closeDisabled} onClick={onClose}>
            ×
          </button>
        </header>
        <div className="body">{children}</div>
        {footer ? <footer>{footer}</footer> : null}
      </div>
    </div>
  );
}
