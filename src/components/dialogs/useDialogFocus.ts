import { useEffect, type RefObject } from "react";

/**
 * Hält den Fokus im Dialog, wenn das fokussierte Element verschwindet oder gesperrt wird (Zustandswechsel
 * des Vergleichs): sonst erreichten Esc und Tab den Dialog nicht mehr. Der Fokus geht auf `data-autofocus`,
 * sonst auf den Dialog selbst; ein Fokus, der noch im Dialog liegt, bleibt unberührt.
 */
export function useDialogFocus(anchor: RefObject<HTMLElement | null>, trigger: unknown): void {
  useEffect(() => {
    const dialog = anchor.current?.closest<HTMLElement>('[role="dialog"], [role="alertdialog"]');
    if (!dialog) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && dialog.contains(active) && !active.matches(":disabled")) return;
    (dialog.querySelector<HTMLElement>("[data-autofocus]:not(:disabled)") ?? dialog).focus();
  }, [anchor, trigger]);
}
