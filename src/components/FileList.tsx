import { memo, useEffect, useRef, type DragEvent, type MouseEvent } from "react";
import { useAppState, useController, useLanguage, useT } from "../app/context";
import { activeTabOf, visibleEntries, type WinIdx } from "../app/state";
import { createTranslator, formatBytes, formatDate, type Language } from "../i18n";
import type { Entry } from "../ipc/types";
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
}

const Row = memo(function Row({ w, index, entry, selected, current, language, extCol }: RowProps) {
  const controller = useController();
  const isDir = entry.kind === "dir";
  const classes = ["row", entry.name === ".." ? "parent-entry" : "", isDir ? "d" : "", entry.kind === "symlink" ? "l" : "", entry.brokenLink ? "broken" : "", entry.hidden ? "hid" : "", entry.error ? "err" : "", selected ? "sel" : "", current ? "cur" : "", extCol ? "ext-split" : ""]
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
  useEffect(() => {
    const list = ref.current;
    if (!list || entries.length === 0) return;
    const row = document.getElementById(rowId(w, focusIndex));
    row?.scrollIntoView?.({ block: "nearest" });
    const rowHeight = row?.getBoundingClientRect().height ?? 0;
    if (rowHeight > 0 && list.clientHeight > 0) controller.setPageSize(w, Math.floor(list.clientHeight / rowHeight) - 1);
  }, [controller, entries, focusIndex, w]);

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
      aria-rowcount={entries.length + 1}
      aria-activedescendant={entries.length > 0 ? rowId(w, focusIndex) : undefined}
      onFocus={() => controller.setActive(w)}
      onContextMenu={(event) => {
        event.preventDefault();
        controller.openContextMenu(w, null, event.clientX, event.clientY);
      }}
    >
      <div role="row" aria-rowindex={1} className={extCol ? "head ext-split" : "head"}>
        {(extCol ? COLUMNS_EXT : COLUMNS).map((column) => {
          const sorted = spec.key === column.key;
          const name = tr(column.label);
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
