import { memo, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent, type KeyboardEvent, type MouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { useAppState, useController, useLanguage, useT } from "../app/context";
import { activeTabOf, visibleEntries, type WinIdx } from "../app/state";
import { createTranslator, formatBytes, formatDate, type Language } from "../i18n";
import type { Entry } from "../ipc/types";
import { clampColumn, COLUMN_DEFAULTS, COLUMN_MAX, COLUMN_MIN, COLUMN_STEP, allWidthsFor, columnOrderFor, defaultOrder, orderedTemplates, OWNER_DEFAULTS, type AllWidths, type ColumnKey } from "../state/columns";
import { isArchivePath } from "../state/paths";
import { fileType, visibleSort, type SortKey } from "../state/sort";
import { formatPermissions } from "../state/permissions";
import { ListBody } from "./ListBody";
import { EntryIcon, Icon } from "./Icon";

const COLUMNS: { key: SortKey; label: "column.name" | "column.size" | "column.type" | "column.ext" | "column.modified" | "column.permissions" | "column.uid" | "column.gid" }[] = [
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

const OWNER_COLUMNS: typeof COLUMNS = [
  { key: "permissions", label: "column.permissions" },
  { key: "uid", label: "column.uid" },
  { key: "gid", label: "column.gid" },
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
  order: ColumnKey[];
  local: boolean;
  dropInto: boolean;
}

const Row = memo(function Row({ w, index, entry, selected, current, language, extCol, order, local, dropInto }: RowProps) {
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
      <span data-column="size" key="size" role="gridcell" className="num tabular-nums">
        {isDir || entry.name === ".." ? "" : formatBytes(entry.size, language)}
      </span>
    ),
    type: (
      <span data-column={extCol ? "ext" : "type"} key="type" role="gridcell" className="ex">
        {extCol ? ext : typeLabel(entry, language)}
      </span>
    ),
    modified: (
      <span data-column="modified" key="modified" role="gridcell" className="dt tabular-nums">
        {entry.name === ".." ? "" : formatDate(entry.modified, language)}
      </span>
    ),
    permissions: <span key="permissions" data-column="permissions" role="gridcell" className="owner permissions" title={entry.permissions ?? formatPermissions(entry.mode)}>{entry.name === ".." ? "" : entry.permissions ?? formatPermissions(entry.mode)}</span>,
    uid: <span key="uid" data-column="uid" role="gridcell" className="owner" title={entry.uid == null ? undefined : `uid ${entry.uid}`}>{entry.name === ".." ? "" : (local ? entry.ownerName : null) ?? entry.uid ?? ""}</span>,
    gid: <span key="gid" data-column="gid" role="gridcell" className="owner" title={entry.gid == null ? undefined : `gid ${entry.gid}`}>{entry.name === ".." ? "" : (local ? entry.groupName : null) ?? entry.gid ?? ""}</span>,
  };
  const overFolder = (event: DragEvent) => {
    if (!controller.dragOverFolder(w, index, event.shiftKey, event.clientX, event.clientY)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.dataTransfer) event.dataTransfer.dropEffect = event.shiftKey ? "move" : "copy";
  };
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
            // WebKit (Tauri unter Linux) macht die Zeile nur zum Ablageziel, wenn auch dragenter abgebrochen wird.
            onDragEnter: overFolder,
            onDragOver: overFolder,
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
  const settings = useAppState(state => state.settings);
  const owners = useMemo(() => [settings.showPermissionsColumn === true, settings.showUidColumn === true, settings.showGidColumn === true],
    [settings.showPermissionsColumn, settings.showUidColumn, settings.showGidColumn]);
  const hasOwners = owners.some(Boolean);
  const extCol = useAppState((state) => state.settings.extensionColumn === true);
  const dropFolder = useAppState((state) => (state.drag?.folder?.w === w ? state.drag.folder.name : null));
  const mode = extCol ? "ext" : "normal";
  const savedWidths = allWidthsFor(settings, mode);
  const defaults = [...COLUMN_DEFAULTS[mode], ...OWNER_DEFAULTS];
  const order = useMemo(() => columnOrderFor(settings, mode), [settings.columnOrder, mode]);
  const visibleOrder = useMemo(() => order.filter(k => defaultOrder(mode).indexOf(k) < 3 || owners[defaultOrder(mode).indexOf(k) - 3]), [order, mode, owners]);
  const columns = [...(extCol ? COLUMNS_EXT : COLUMNS), ...OWNER_COLUMNS];
  const shownColumns = [columns[0]!, ...visibleOrder.map(k => columns.find(c => c.key === k)!)];
  const saveWidths = (next: AllWidths) => controller.updateSettings(s => ({ ...s,
    columnWidths: { ...s.columnWidths, [mode]: next.slice(0, 3) as [number, number, number] },
    ownerColumnWidths: { ...s.ownerColumnWidths, [mode]: next.slice(3) as [number, number, number] },
  }));
  // Entwurf während des Ziehens bzw. Tastenanpassens; erst beim Loslassen wird gespeichert.
  const [draft, setDraft] = useState<AllWidths | null>(null);
  const [dragSlot, setDragSlot] = useState<number | null>(null);
  const draftRef = useRef<AllWidths | null>(null);
  const widths = draft ?? savedWidths;
  const setLive = (next: AllWidths | null) => {
    draftRef.current = next;
    setDraft(next);
  };
  const commitWidths = () => {
    const next = draftRef.current;
    setLive(null);
    if (!next) return;
    saveWidths(next);
  };
  const widthAt = (slot: number, value: number): AllWidths => {
    const next: AllWidths = [...(draftRef.current ?? savedWidths)];
    next[slot] = clampColumn(value, defaults[slot]!);
    return next;
  };
  const resetWidth = (slot: number) => {
    setLive(null);
    saveWidths(widthAt(slot, defaults[slot]!));
  };
  const startDrag = (slot: number, event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    cleanupGesture.current?.();
    const startX = event.clientX;
    const pointerId = event.pointerId;
    const startWidth = widths[slot]!;
    setDragSlot(slot);
    // The left-edge handle widens its column when dragged left.
    const move = (e: PointerEvent) => {
      if (e.pointerId === pointerId) setLive(widthAt(slot, startWidth + startX - e.clientX));
    };
    const cleanup = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("blur", cancel);
      setDragSlot(null);
      cleanupGesture.current = null;
    };
    const cancel = () => { cleanup(); setLive(null); };
    const end = (e: PointerEvent) => {
      if (e.pointerId !== pointerId) return;
      cleanup();
      commitWidths();
    };
    cleanupGesture.current = cancel;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("blur", cancel);
  };
  const onHandleKey = (slot: number, event: KeyboardEvent<HTMLElement>) => {
    // Let native Tab traversal reach the next header instead of the global pane shortcut.
    if (event.key === "Tab") event.stopPropagation();
    const delta = event.key === "ArrowLeft" ? COLUMN_STEP : event.key === "ArrowRight" ? -COLUMN_STEP : 0;
    if (delta === 0) return;
    event.preventDefault();
    event.stopPropagation();
    setLive(widthAt(slot, (draftRef.current ?? savedWidths)[slot]! + delta));
  };
  const [announcement, setAnnouncement] = useState("");
  const [insertion, setInsertion] = useState<{ key: ColumnKey; after: boolean } | null>(null);
  const suppressClick = useRef(false);
  const cleanupGesture = useRef<(() => void) | null>(null);
  useEffect(() => () => cleanupGesture.current?.(), [mode, settings.columnOrder, owners]);
  const saveOrder = (next: ColumnKey[], key: ColumnKey) => {
    controller.updateSettings(s => ({ ...s, columnOrder: { ...s.columnOrder, [mode]: next } }));
    setAnnouncement(tr("column.moved", { column: tr(columns.find(c => c.key === key)!.label), position: next.indexOf(key) + 1 }));
  };
  const reorderKey = (key: ColumnKey, event: KeyboardEvent<HTMLButtonElement>) => {
    if (!event.altKey || !["ArrowLeft", "ArrowRight", "Home"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Home") {
      controller.updateSettings(s => ({ ...s, columnOrder: { ...s.columnOrder, [mode]: defaultOrder(mode) } }));
      setAnnouncement(tr("column.orderReset"));
      return;
    }
    const index = visibleOrder.indexOf(key);
    const target = visibleOrder[index + (event.key === "ArrowLeft" ? -1 : 1)];
    if (!target) return;
    const next = order.filter(k => k !== key);
    next.splice(next.indexOf(target) + (event.key === "ArrowRight" ? 1 : 0), 0, key);
    saveOrder(next, key);
  };
  const startReorder = (key: ColumnKey, event: ReactPointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return;
    cleanupGesture.current?.();
    suppressClick.current = false;
    const startX = event.clientX, startY = event.clientY;
    const pointerId = event.pointerId;
    const button = event.currentTarget;
    let dragging = false;
    let target: { key: ColumnKey; after: boolean } | null = null;
    const move = (e: PointerEvent) => {
      if (e.pointerId !== pointerId) return;
      if (!dragging && Math.hypot(e.clientX - startX, e.clientY - startY) < 6) return;
      dragging = true;
      suppressClick.current = true;
      target = null;
      for (const header of ref.current?.querySelectorAll<HTMLElement>('[role="columnheader"]') ?? []) {
        const rect = header.getBoundingClientRect();
        const id = header.dataset.column === "ext" ? "type" : header.dataset.column;
        if (id === "name" || rect.width === 0) continue;
        if (e.clientX >= rect.left && e.clientX <= rect.right && e.clientY >= rect.top && e.clientY <= rect.bottom) {
          target = { key: id as ColumnKey, after: e.clientX > rect.left + rect.width / 2 };
          break;
        }
      }
      setInsertion(target);
    };
    const cleanup = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("blur", cancel);
      setInsertion(null);
      cleanupGesture.current = null;
    };
    const cancel = () => { cleanup(); };
    const end = (e: PointerEvent) => {
      if (e.pointerId !== pointerId) return;
      cleanup();
      if (dragging && target && target.key !== key) {
        const next = order.filter(k => k !== key);
        next.splice(next.indexOf(target.key) + (target.after ? 1 : 0), 0, key);
        saveOrder(next, key);
      }
      button.focus();
    };
    cleanupGesture.current = cleanup;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("blur", cancel);
  };
  const spec = visibleSort(sort, settings);
  const ref = useRef<HTMLDivElement>(null);
  const focusIndex = Math.min(selection.focus, Math.max(0, entries.length - 1));

  // Fokus der Seite folgt dem aktiven Fenster (außer in einem Eingabefeld oder Dialog).
  useEffect(() => {
    if (!active) return;
    const current = document.activeElement;
    const typing = current instanceof HTMLInputElement || current instanceof HTMLTextAreaElement;
    const inDialog = current instanceof HTMLElement && current.closest('[role="dialog"], [role="alertdialog"], [role="menu"]');
    if (!typing && !inDialog && !ref.current?.contains(current)) ref.current?.focus({ preventScroll: true });
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
      className={hasOwners ? "list owner-columns adjustable-columns" : "list adjustable-columns"}
      role="grid"
      tabIndex={0}
      aria-label={`${tr("pane.listLabel")} – ${tr(w === 0 ? "pane.left" : "pane.right")}`}
      aria-multiselectable="true"
      style={orderedTemplates(widths, mode, order, owners) as CSSProperties}
      aria-rowcount={entries.length + 1}
      aria-activedescendant={entries.length > 0 ? rowId(w, focusIndex) : undefined}
      onFocus={() => controller.setActive(w)}
      onContextMenu={(event) => {
        event.preventDefault();
        controller.openContextMenu(w, null, event.clientX, event.clientY);
      }}
    >
      <div role="row" aria-rowindex={1} className={extCol ? "head ext-split" : "head"}>
        {shownColumns.map((column) => {
          const sorted = spec.key === column.key;
          const name = tr(column.label);
          const slot = defaultOrder(mode).indexOf(column.key as ColumnKey);
          return (
            <span
              key={column.key}
              data-column={column.key === "type" && extCol ? "ext" : column.key}
              data-insertion={insertion?.key === column.key ? (insertion.after ? "after" : "before") : undefined}
              role="columnheader"
              aria-sort={sorted ? (spec.direction === "asc" ? "ascending" : "descending") : "none"}
              className={[sorted ? "s" : "", column.key === "size" || column.key === "modified" ? "numeric tabular-nums" : ""].join(" ")}
            >
              <button
                type="button"
                tabIndex={0}
                aria-description={slot >= 0 ? tr("column.reorderHint") : undefined}
                onPointerDown={slot >= 0 ? event => startReorder(column.key as ColumnKey, event) : undefined}
                onKeyDown={slot >= 0 ? event => reorderKey(column.key as ColumnKey, event) : undefined}
                title={tr("tip.sortBy", { column: name }) + (slot >= 0 ? ". " + tr("column.reorderHint") : "")}
                onMouseDown={(event) => event.preventDefault()}
                onClick={(event) => { if (event.detail > 0 && suppressClick.current) { suppressClick.current = false; return; } controller.sortBy(w, column.key); }}
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
      <span className="sr-only" aria-live="polite" aria-atomic="true">{announcement}</span>
      <ListBody w={w}>
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
            order={visibleOrder}
            local={listing.path?.startsWith("/") === true}
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
      </ListBody>
    </div>
  );
}
