import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { useAppState, useController, useLanguage, useRuntime, useT } from "../app/context";
import type { WinIdx } from "../app/state";
import { formatBytes, formatDate } from "../i18n";
import type { TrashEntry } from "../ipc/types";
import { ListBody } from "./ListBody";
import { Icon } from "./Icon";

const COLUMNS = "minmax(0, 1.6fr) minmax(0, 1.6fr) 130px 80px";

/** The trash shown inside a pane (virtual tab): list, restore, delete for good. Local only. */
export function TrashView({ w }: { w: WinIdx }) {
  const { api } = useRuntime();
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const active = useAppState(state => state.active === w);
  const [list, setList] = useState<{ entries: TrashEntry[]; truncated: boolean } | null>(null);
  const [error, setError] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [cursor, setCursor] = useState(0);
  const [anchor, setAnchor] = useState(0);
  const gridRef = useRef<HTMLDivElement>(null);
  const entries = list?.entries ?? [];

  const load = useCallback(() => {
    api.trashList().then(
      result => { setList(result); setError(false); setSelected(old => new Set(result.entries.filter(e => old.has(e.id)).map(e => e.id))); setCursor(c => Math.min(c, Math.max(0, result.entries.length - 1))); },
      () => { setError(true); setList({ entries: [], truncated: false }); },
    );
  }, [api]);
  useEffect(() => {
    load();
    document.addEventListener("dualfile:trash-changed", load);
    return () => document.removeEventListener("dualfile:trash-changed", load);
  }, [load]);

  const targets = (): TrashEntry[] => selected.size ? entries.filter(e => selected.has(e.id)) : entries.slice(cursor, cursor + 1);
  const restore = () => void controller.trashRestore(targets());
  const purge = () => controller.askTrashPurge(targets());
  const pick = (index: number, mode: "single" | "toggle" | "range") => {
    const entry = entries[index];
    if (!entry) return;
    if (mode === "range") setSelected(new Set(entries.slice(Math.min(anchor, index), Math.max(anchor, index) + 1).map(e => e.id)));
    else if (mode === "toggle") setSelected(old => { const next = new Set(old); if (!next.delete(entry.id)) next.add(entry.id); return next; });
    else setSelected(new Set([entry.id]));
    if (mode !== "range") setAnchor(index);
    setCursor(index);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const last = entries.length - 1;
    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      const row = document.getElementById(`trash-${w}-${cursor}`);
      const box = (row ?? gridRef.current)?.getBoundingClientRect();
      controller.setActive(w);
      setMenu({ x: (box?.left ?? 0) + 24, y: box?.bottom ?? 80, background: !row });
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      const next = event.key === "Home" ? 0 : event.key === "End" ? last : Math.max(0, Math.min(last, cursor + (event.key === "ArrowDown" ? 1 : -1)));
      if (event.shiftKey) pick(next, "range");
      else if (event.ctrlKey) setCursor(next);
      else pick(next, "single");
    } else if (event.key === " " && entries[cursor]) pick(cursor, "toggle");
    else if (event.key === "Delete") purge();
    else if (event.key === "Enter" && (event.ctrlKey || event.altKey)) restore();
    else if (event.key === "a" && event.ctrlKey) setSelected(new Set(entries.map(e => e.id)));
    else return;
    event.preventDefault();
    event.stopPropagation();
  };
  const onContextMenu = (event: MouseEvent, index: number) => {
    event.preventDefault();
    event.stopPropagation();
    if (!selected.has(entries[index]!.id)) pick(index, "single");
    controller.setActive(w);
    setMenu({ x: event.clientX, y: event.clientY });
  };
  const [menu, setMenu] = useState<{ x: number; y: number; background?: boolean } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => { (menuRef.current?.querySelector<HTMLElement>("button:not(:disabled)") ?? menuRef.current)?.focus(); }, [menu]);
  const closeMenu = () => { setMenu(null); gridRef.current?.focus(); };
  const empty = list !== null && entries.length === 0;
  const has = entries.length > 0;
  const info = `${tr("trash.count", { count: entries.length })}${selected.size ? ` · ${tr("trash.marked", { count: selected.size })}` : ""}`;

  return (
    <>
      <div className="crumbs trash-path"><span>{tr("trash.title")}</span></div>
      <div className="trash-bar">
        <button type="button" className="tb primary" disabled={!has} onClick={restore}>{tr("trash.restore")}</button>
        <button type="button" className="tb danger" disabled={!has} onClick={purge}>{tr("trash.purge")}</button>
        <button type="button" className="tb danger" disabled={!has} onClick={() => controller.openDialog({ kind: "placeConfirm", action: "emptyTrash" })}>{tr("trash.empty")}</button>
        <span className="trash-info" role="status">{info}</span>
      </div>
      <div ref={gridRef} className="list" role="grid" aria-label={tr("trash.title")} aria-multiselectable="true" aria-rowcount={has ? entries.length + 1 : 0}
        aria-activedescendant={has ? `trash-${w}-${cursor}` : undefined} tabIndex={0} onContextMenu={event => {
          event.preventDefault(); event.stopPropagation(); controller.setActive(w);
          setMenu({ x: event.clientX, y: event.clientY, background: true });
        }} onKeyDown={onKeyDown} onFocus={() => controller.setActive(w)}>
        {has ? <div role="row" aria-rowindex={1} className="head tv" style={{ "--df-cols": COLUMNS } as React.CSSProperties}>
          <span role="columnheader">{tr("column.name")}</span><span role="columnheader">{tr("trash.colOriginal")}</span>
          <span role="columnheader">{tr("trash.colDeleted")}</span><span role="columnheader" className="num">{tr("column.size")}</span>
        </div> : null}
        <ListBody w={w}>
          {error ? <div className="trash-empty" role="alert">{tr("sidebar.trashUnavailable")}</div> : null}
          {empty && !error ? <div className="trash-empty"><div><Icon name="trash" size={28} /><b>{tr("trash.emptyTitle")}</b>{tr("trash.emptyHint")}</div></div> : null}
          {entries.map((entry, index) => (
            <div key={entry.id} id={`trash-${w}-${index}`} role="row" aria-rowindex={index + 2} aria-selected={selected.has(entry.id)}
              className={`row tv${entry.isDir ? " d" : ""}${selected.has(entry.id) ? " sel" : ""}${index === cursor && active ? " cur" : ""}`}
              style={{ "--df-cols": COLUMNS } as React.CSSProperties}
              onClick={event => pick(index, event.shiftKey ? "range" : event.ctrlKey || event.metaKey ? "toggle" : "single")}
              onContextMenu={event => onContextMenu(event, index)}>
              <span role="gridcell" className="n"><Icon name={entry.isDir ? "dir" : "file"} /><span className="nm" title={entry.name}>{entry.name}</span></span>
              <span role="gridcell" className="dt orig" title={entry.originalParent}><bdi>{entry.originalParent}</bdi></span>
              <span role="gridcell" className="dt tabular-nums">{formatDate(entry.deletedAt === null ? null : entry.deletedAt * 1000, language)}</span>
              <span role="gridcell" className="num tabular-nums">{entry.size === null ? "" : formatBytes(entry.size, language)}</span>
            </div>
          ))}
        </ListBody>
      </div>
      {list?.truncated ? <div className="foot" role="status">{tr("trash.truncated", { count: entries.length })}</div> : null}
      {menu ? (
        <div className="menu-backdrop" onMouseDown={closeMenu} onContextMenu={event => { event.preventDefault(); closeMenu(); }}>
          <div ref={menuRef} tabIndex={-1} className="ctx" role="menu" aria-label={tr("ui.contextMenu")} style={{ left: Math.max(4, Math.min(menu.x, window.innerWidth - 220)), top: Math.max(4, Math.min(menu.y, window.innerHeight - 140)) }}
            onMouseDown={event => event.stopPropagation()}
            onKeyDown={event => {
              const buttons = [...event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled)")];
              const i = buttons.indexOf(document.activeElement as HTMLElement);
              if (event.key === "ArrowDown") buttons[(i + 1) % buttons.length]?.focus();
              else if (event.key === "ArrowUp") buttons[(i - 1 + buttons.length) % buttons.length]?.focus();
              else if (event.key === "Home") buttons[0]?.focus();
              else if (event.key === "End") buttons.at(-1)?.focus();
              else if (event.key === "Escape" || event.key === "Tab") closeMenu();
              else return;
              event.preventDefault();
              event.stopPropagation();
            }}>
            {menu.background ? <button type="button" role="menuitem" disabled={!has} aria-disabled={!has} aria-description={!has ? tr("trash.emptyTitle") : undefined} onClick={() => { closeMenu(); controller.openDialog({ kind: "placeConfirm", action: "emptyTrash" }); }}>{tr("trash.empty")}{!has && <small className="menu-reason">{tr("trash.emptyTitle")}</small>}</button> : <>
            <button type="button" role="menuitem" onClick={() => { closeMenu(); restore(); }}>{tr("trash.restore")}</button>
            <button type="button" role="menuitem" disabled={targets().length !== 1} aria-disabled={targets().length !== 1} aria-description={targets().length !== 1 ? tr("menu.reason.single") : undefined} onClick={() => { closeMenu(); const [item] = targets(); if (item) controller.openTrashOriginal(w, item); }}>{tr("trash.openOriginal")}{targets().length !== 1 && <small className="menu-reason">{tr("menu.reason.single")}</small>}</button>
            <div role="separator" className="header-menu-separator" />
            <button className="danger" type="button" role="menuitem" onClick={() => { closeMenu(); purge(); }}>{tr("trash.purge")}</button>
            </>}
          </div>
        </div>
      ) : null}
    </>
  );
}
