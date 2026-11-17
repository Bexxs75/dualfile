import { memo, useEffect, useRef, useState, type CSSProperties, type DragEvent, type KeyboardEvent, type MouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { useAppState, useController, useLanguage, useT } from "../app/context";
import { activeTabOf, visibleEntries, type WinIdx } from "../app/state";
import { createTranslator, formatBytes, formatDate, type Language } from "../i18n";
import type { Entry } from "../ipc/types";
import { clampColumn, COLUMN_DEFAULTS, COLUMN_MAX, COLUMN_MIN, COLUMN_STEP, columnTemplate, columnWidthsFor, type ColumnTriple } from "../state/columns";
import { isArchivePath } from "../state/paths";
import { fileType, parseSort, type SortKey } from "../state/sort";
import { EntryIcon, Icon } from "./Icon";

const COLUMNS: { key: SortKey; label: "column.name" | "column.size" | "column.type" | "column.ext" | "column.modified" }[] = [
  { key: "name", label: "column.name" },
  { key: "size", label: "column.size" },
  { key: "type", label: "column.type" },
  { key: "modified", label: "column.modified" },
];

/** Total-Commander-Ansicht: Endung als eigene Spalte direkt hinter dem Namen. */
const COLUMNS_EXT: typeof COLUMNS = [
  { key: "name", label: "column.name" },
  { key: "type", label: "column.ext" },
  { key: "size", label: "column.size" },
  { key: "modified", label: "column.modified" },
];

export const rowId = (w: WinIdx, index: number): string => `pane${w}-row-${index}`;

function typeLabel(entry: Entry, language: Language): string {
  const tr = createTranslator(language);
  if (entry.name === "..") return tr("type.up");
  if (entry.kind === "dir") return tr("type.folder");
  if (entry.kind === "symlink") return tr(entry.brokenLink ? "type.brokenLink" : "type.link");
  const extension = fileType(entry.name);
  return extension === "" ? tr("type.file") : tr("type.fileWithExtension", { extension: extension.toUpperCase() });
}

interface RowProps {
  w: WinIdx;
  index: number;
  entry: Entry;
  selected: boolean;
  current: boolean;
  language: Language;
  extCol: boolean;
  dropInto: boolean;
}

const Row = memo(function Row({ w, index, entry, selected, current, language, extCol, dropInto }: RowProps) {
  const controller = useController();
  const isDir = entry.kind === "dir";
  const classes = ["row", entry.name === ".." ? "parent-entry" : "", isDir ? "d" : "", entry.kind === "symlink" ? "l" : "", entry.brokenLink ? "broken" : "", entry.hidden ? "hid" : "", entry.error ? "err" : "", selected ? "sel" : "", current ? "cur" : "", extCol ? "ext-split" : "", dropInto ? "drop-into" : ""]
    .filter(Boolean)
    .join(" ");
  const draggable = entry.name !== "..";
  const ext = extCol && !isDir && entry.name !== ".." ? fileType(entry.name) : "";
  const shown = ext !== "" ? entry.name.slice(0, entry.name.lastIndexOf(".")) : entry.name;
  const nameEnd = Math.min(16, Math.ceil(shown.length / 2));
  const cells = {
    size: (
      <span key="size" role="gridcell" className="num tabular-nums">
        {isDir || entry.name === ".." ? "" : formatBytes(entry.size, language)}
      </span>
    ),
    type: (
      <span key="type" role="gridcell" className="ex">
        {extCol ? ext : typeLabel(entry, language)}
      </span>
    ),
    modified: (
      <span key="modified" role="gridcell" className="dt tabular-nums">
        {entry.name === ".." ? "" : formatDate(entry.modified, language)}
      </span>
    ),
  };
  const order = extCol ? (["type", "size", "modified"] as const) : (["size", "type", "modified"] as const);
  return (
    <div
      id={rowId(w, index)}
      role="row"
      aria-rowindex={index + 2}
      aria-selected={selected}
      className={classes}
      title={entry.error ?? (entry.linkTarget ? `→ ${entry.linkTarget}` : undefined)}
      draggable={draggable}
      onClick={(event: MouseEvent) => controller.clickRow(w, index, { ctrl: event.ctrlKey || event.metaKey, shift: event.shiftKey })}
      onDoubleClick={() => controller.activateRow(w, index)}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        controller.openContextMenu(w, index, event.clientX, event.clientY);
      }}
      onDragStart={(event: DragEvent) => {
        if (event.dataTransfer) {
          event.dataTransfer.effectAllowed = "copyMove";
          event.dataTransfer.setData("text/plain", entry.name);
        }
        controller.dragStart(w, index);
      }}
      onDragEnd={() => controller.dragEnd()}
      {...(isDir && entry.name !== ".."
        ? {
            // Ordnerzeilen sind Ablageziele, auch im eigenen Fenster; sonst übernimmt das Fenster (aktueller Ordner).
            onDragOver: (event: DragEvent) => {
              if (!controller.dragOverFolder(w, index, event.shiftKey, event.clientX, event.clientY)) return;
              event.preventDefault();
              event.stopPropagation();
              if (event.dataTransfer) event.dataTransfer.dropEffect = event.shiftKey ? "move" : "copy";
            },
            onDrop: (event: DragEvent) => {
              if (!controller.dropOnFolder(w, index, event.shiftKey)) return;
              event.preventDefault();
              event.stopPropagation();
            },
          }
        : {})}
    >
      <span role="gridcell" className="n">
        <EntryIcon entry={entry} />
        <span className="nm" title={entry.name}><span className="nm-start">{shown.slice(0, shown.length - nameEnd)}</span><span className="nm-end">{shown.slice(shown.length - nameEnd)}</span></span>
        {entry.linkTarget ? <span className="lt"> → {entry.linkTarget}</span> : null}
      </span>
      {order.map((key) => cells[key])}
    </div>
  );
});

/** Die Dateiliste eines Fensters als ARIA-Tabelle: Kopfzeile mit Sortierung, Zeilen mit Markierung und Fokus. */
export function FileList({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const entries = useAppState((state) => visibleEntries(state, w));
  const selection = useAppState((state) => activeTabOf(state, w).selection);
  const sort = useAppState((state) => activeTabOf(state, w).sort);
  const listing = useAppState((state) => state.listings[w]);
  const filter = useAppState((state) => activeTabOf(state, w).filter);
  const active = useAppState((state) => state.active === w);
  const extCol = useAppState((state) => state.settings.extensionColumn === true);
  const dropFolder = useAppState((state) => (state.drag?.folder?.w === w ? state.drag.folder.name : null));
  const storedWidths = useAppState((state) => state.settings.columnWidths);
  const mode = extCol ? "ext" : "normal";
  const savedWidths = columnWidthsFor({ columnWidths: storedWidths }, mode);
  // Entwurf während des Ziehens bzw. Tastenanpassens; erst beim Loslassen wird gespeichert.
  const [draft, setDraft] = useState<ColumnTriple | null>(null);
  const [dragSlot, setDragSlot] = useState<number | null>(null);
  const draftRef = useRef<ColumnTriple | null>(null);
  const widths = draft ?? savedWidths;
  const setLive = (next: ColumnTriple | null) => {
    draftRef.current = next;
    setDraft(next);
  };
  const commitWidths = () => {
    const next = draftRef.current;
    setLive(null);
    if (!next) return;
    controller.updateSettings((s) => ({ ...s, columnWidths: { ...s.columnWidths, [mode]: next } }));
  };
  const widthAt = (slot: number, value: number): ColumnTriple => {
    const next: ColumnTriple = [...(draftRef.current ?? savedWidths)];
    next[slot] = clampColumn(value, COLUMN_DEFAULTS[mode][slot]);
    return next;
  };
  const resetWidth = (slot: number) => {
    setLive(null);
    controller.updateSettings((s) => ({ ...s, columnWidths: { ...s.columnWidths, [mode]: widthAt(slot, COLUMN_DEFAULTS[mode][slot]) } }));
  };
  const startDrag = (slot: number, event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const startX = event.clientX;
    const startWidth = widths[slot];
    setDragSlot(slot);
    // Handle sits on the column's LEFT edge; dragging it left widens the column so the
    // divider tracks the cursor (the flexible name column on the left absorbs the change).
    const move = (e: PointerEvent) => setLive(widthAt(slot, startWidth + startX - e.clientX));
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      setDragSlot(null);
      commitWidths();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  };
  const onHandleKey = (slot: number, event: KeyboardEvent<HTMLElement>) => {
    const delta = event.key === "ArrowLeft" ? COLUMN_STEP : event.key === "ArrowRight" ? -COLUMN_STEP : 0;
    if (delta === 0) return;
    event.preventDefault();
    event.stopPropagation();
    setLive(widthAt(slot, (draftRef.current ?? savedWidths)[slot] + delta));
  };
  const spec = parseSort(sort);
  const ref = useRef<HTMLDivElement>(null);
  const focusIndex = Math.min(selection.focus, Math.max(0, entries.length - 1));

  // Fokus der Seite folgt dem aktiven Fenster (außer in einem Eingabefeld oder Dialog).
  useEffect(() => {
    if (!active) return;
    const current = document.activeElement;
    const typing = current instanceof HTMLInputElement || current instanceof HTMLTextAreaElement;
    const inDialog = current instanceof HTMLElement && current.closest('[role="dialog"], [role="alertdialog"], [role="menu"]');
    if (!typing && !inDialog) ref.current?.focus({ preventScroll: true });
  }, [active]);

  // Fokuszeile sichtbar halten und die Seitengröße für Bild auf/ab messen.
  // Wichtig: nur scrollen, wenn sich der Fokus bewegt hat oder der Ordner wechselte.
  // Sonst würde jede Neuzeichnung (neue entries-Referenz) die manuelle Mausrad-Position
  // in großen Ordnern immer wieder auf die Auswahl zurückreißen.
  const prevFocus = useRef(focusIndex);
  const prevPath = useRef(listing.path);
  useEffect(() => {
    const list = ref.current;
    if (!list || entries.length === 0) return;
    const row = document.getElementById(rowId(w, focusIndex));
    if (prevFocus.current !== focusIndex || prevPath.current !== listing.path) {
      row?.scrollIntoView?.({ block: "nearest" });
      prevFocus.current = focusIndex;
      prevPath.current = listing.path;
    }
    const rowHeight = row?.getBoundingClientRect().height ?? 0;
    if (rowHeight > 0 && list.clientHeight > 0) controller.setPageSize(w, Math.floor(list.clientHeight / rowHeight) - 1);
  }, [controller, entries, focusIndex, w, listing.path]);

  const realCount = entries.filter((e) => e.name !== "..").length;
  const message = listing.error
    ? null
    : listing.loading || (listing.path === null && entries.length === 0)
      ? tr("pane.loading")
      : realCount === 0
        ? filter !== ""
          ? tr("pane.noMatches")
          : tr("pane.empty")
        : null;

  return (
    <div
      ref={ref}
      className="list"
      role="grid"
      tabIndex={0}
      aria-label={`${tr("pane.listLabel")} – ${tr(w === 0 ? "pane.left" : "pane.right")}`}
      aria-multiselectable="true"
      style={{ "--df-cols": columnTemplate(widths) } as CSSProperties}
      aria-rowcount={entries.length + 1}
      aria-activedescendant={entries.length > 0 ? rowId(w, focusIndex) : undefined}
      onFocus={() => controller.setActive(w)}
      onContextMenu={(event) => {
        event.preventDefault();
        controller.openContextMenu(w, null, event.clientX, event.clientY);
      }}
    >
      <div role="row" aria-rowindex={1} className={extCol ? "head ext-split" : "head"}>
        {(extCol ? COLUMNS_EXT : COLUMNS).map((column, columnIndex) => {
          const sorted = spec.key === column.key;
          const name = tr(column.label);
          const slot = columnIndex - 1; // Spalte 0 (Name) ist flexibel und nicht ziehbar
          return (
            <span
              key={column.key}
              role="columnheader"
              aria-sort={sorted ? (spec.direction === "asc" ? "ascending" : "descending") : "none"}
              className={[sorted ? "s" : "", column.key === "size" || column.key === "modified" ? "numeric tabular-nums" : ""].join(" ")}
            >
              <button
                type="button"
                tabIndex={-1}
                title={tr("tip.sortBy", { column: name })}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => controller.sortBy(w, column.key)}
              >
                {name}
                {sorted ? <Icon name={spec.direction === "asc" ? "sortUp" : "down"} size={12} className="arrow" /> : null}
              </button>
              {slot >= 0 ? (
                <span
                  role="separator"
                  aria-orientation="vertical"
                  aria-label={tr("column.resize", { column: name })}
                  aria-valuemin={COLUMN_MIN}
                  aria-valuemax={COLUMN_MAX}
                  aria-valuenow={widths[slot]}
                  title={tr("column.resizeReset")}
                  tabIndex={0}
                  className={dragSlot === slot ? "col-resize dragging" : "col-resize"}
                  onPointerDown={(event) => startDrag(slot, event)}
                  onClick={(event) => event.stopPropagation()}
                  onDoubleClick={(event) => {
                    event.stopPropagation();
                    resetWidth(slot);
                  }}
                  onKeyDown={(event) => onHandleKey(slot, event)}
                  onKeyUp={(event) => {
                    if (event.key === "ArrowLeft" || event.key === "ArrowRight") commitWidths();
                  }}
                  onBlur={commitWidths}
                />
              ) : null}
            </span>
          );
        })}
      </div>
      {entries.map((entry, index) => (
        <Row
          key={entry.name}
          w={w}
          index={index}
          entry={entry}
          selected={selection.selected.has(entry.name)}
          current={index === focusIndex}
          language={language}
          extCol={extCol}
          dropInto={dropFolder === entry.name}
        />
      ))}
      {listing.error ? (
        <div className="pane-msg error" role="alert">
          {listing.error}
          {isArchivePath(listing.path ?? "") ? (
            <button type="button" className="tb small" onMouseDown={(event) => event.preventDefault()} onClick={() => void controller.goUp(w)}>
              {tr("archive.leave")}
            </button>
          ) : null}
        </div>
      ) : message ? (
        <div className="pane-msg">{message}</div>
      ) : null}
    </div>
  );
}
