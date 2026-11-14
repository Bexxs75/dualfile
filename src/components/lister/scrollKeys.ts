import type { KeyboardEvent, WheelEvent } from "react";

export interface PagedNav {
  hasPrev: boolean;
  hasNext: boolean;
  prevPage(): void;
  nextPage(): void;
  /** Anfang bzw. Ende der Datei anzeigen. */
  toStart(): void;
  toEnd(): void;
  lineHeight: number;
}

const atTop = (el: HTMLElement): boolean => el.scrollTop <= 0;
const atBottom = (el: HTMLElement): boolean => el.scrollTop + el.clientHeight >= el.scrollHeight - 1;

/**
 * Blättern mit der Tastatur in einem Fenster (Text/Hex): scrollt innerhalb des geladenen
 * Fensters, am Rand wechselt es zur vorherigen/nächsten Seite. Liefert true, wenn die Taste benutzt wurde.
 */
export function handleScrollKey(event: KeyboardEvent<HTMLElement>, el: HTMLElement, nav: PagedNav): boolean {
  if (event.ctrlKey || event.altKey || event.metaKey) return false;
  const page = Math.max(nav.lineHeight, el.clientHeight * 0.9);
  const down = (distance: number): void => {
    if (atBottom(el) && nav.hasNext) nav.nextPage();
    else el.scrollTop += distance;
  };
  const up = (distance: number): void => {
    if (atTop(el) && nav.hasPrev) nav.prevPage();
    else el.scrollTop -= distance;
  };
  switch (event.key) {
    case "ArrowDown":
      down(nav.lineHeight);
      return true;
    case "ArrowUp":
      up(nav.lineHeight);
      return true;
    case "PageDown":
      down(page);
      return true;
    case "PageUp":
      up(page);
      return true;
    case " ":
      if (event.shiftKey) up(page);
      else down(page);
      return true;
    case "Home":
      if (nav.hasPrev) nav.toStart();
      else el.scrollTop = 0;
      return true;
    case "End":
      if (nav.hasNext) nav.toEnd();
      else el.scrollTop = el.scrollHeight;
      return true;
    default:
      return false;
  }
}

/** Mausrad am Rand des Fensters blättert zur nächsten/vorherigen Seite. */
export function handleWheel(event: WheelEvent<HTMLElement>, nav: PagedNav): void {
  const el = event.currentTarget;
  if (event.deltaY > 0 && atBottom(el) && nav.hasNext) nav.nextPage();
  else if (event.deltaY < 0 && atTop(el) && nav.hasPrev) nav.prevPage();
}
