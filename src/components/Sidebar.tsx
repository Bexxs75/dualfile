import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useAppState, useController, useT } from "../app/context";
import { activeTabOf } from "../app/state";
import { activePlace, buildPlaces, type Place } from "../state/places";
import { Icon } from "./Icon";

/** Geometry changes are serialized; automatic narrowing never changes saved preferences. */
export function SidebarLayout({ children }: { children: ReactNode }) {
  const controller = useController();
  const tr = useT();
  const mode = useAppState(s => s.settings.sidebarMode ?? "full");
  const width = useAppState(s => s.settings.sidebarWidth ?? 210);
  const root = useRef<HTMLDivElement>(null);
  const desired = mode === "hidden" ? 0 : mode === "icons" ? 44 : width;
  const [shown, setShown] = useState(desired);
  // Geometry credit records only confirmed growth (including restored window width).
  // A refused shrink leaves that credit intact; a refused grow earns no credit.
  const grown = useRef(0);
  const queue = useRef(Promise.resolve());
  const busy = useRef(false);
  const mounted = useRef(true);
  const latest = useRef({ desired, mode, width }); latest.current = { desired, mode, width };
  const available = () => root.current?.closest<HTMLElement>(".app")?.clientWidth || window.innerWidth;
  const fallback = () => latest.current.mode === "full" && available() < latest.current.width + 920 ? 44 : latest.current.desired;
  const reconcile = useRef(() => {});
  reconcile.current = () => {
    queue.current = queue.current.then(async () => {
      if (!mounted.current) return;
      busy.current = true;
      const request = latest.current;
      let target = request.desired;
      try {
        const delta = request.desired - grown.current;
        if (delta !== 0) {
          const result = await controller.api.sidebarResizeWindow(delta);
          grown.current = Math.max(0, grown.current + result.actualDelta + (result.restoredWidth ?? 0));
          if (request.mode === "full" && !result.canGrow && available() < request.width + 920) target = 44;
          const excess = grown.current - target;
          if (target !== request.desired && excess > 0) {
            const correction = await controller.api.sidebarResizeWindow(-Math.min(400, excess));
            grown.current = Math.max(0, grown.current + correction.actualDelta);
          }
        } else target = fallback();
      } catch { target = fallback(); }
      if (mounted.current) setShown(target);
      busy.current = false;
    });
  };
  const previousMode = useRef(mode);
  const initial = useRef(true);
  useEffect(() => {
    const immediate = initial.current || previousMode.current !== mode;
    initial.current = false; previousMode.current = mode;
    if (immediate) { reconcile.current(); return; }
    // Coalesce keyboard repeats and settings input edits into one compositor request.
    const timer = setTimeout(() => reconcile.current(), 200);
    return () => clearTimeout(timer);
  }, [desired, mode, controller]);
  useEffect(() => {
    mounted.current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    // User resizing only re-evaluates the rail fallback; it never resizes the OS window.
    const measure = () => { if (busy.current) return; clearTimeout(timer); timer = setTimeout(() => setShown(fallback()), 150); };
    const element = root.current?.closest<HTMLElement>(".app");
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    if (element) observer?.observe(element);
    window.addEventListener("resize", measure);
    return () => { mounted.current = false; clearTimeout(timer); observer?.disconnect(); window.removeEventListener("resize", measure); };
  }, []);
  const effective = shown === 0 ? "hidden" : shown === 44 ? "icons" : "full";
  return <div className="body-row" ref={root}><Sidebar mode={effective} width={shown || width} /><div className="panes">{children}</div><span className="sr-only" role="status" aria-live="polite">{tr("sidebar.title")}: {tr(`sidebar.${effective}`)}</span></div>;
}

export function Sidebar({ mode, width }: { mode: "hidden" | "icons" | "full"; width: number }) {
  const state = useAppState(s => s), controller = useController(), tr = useT();
  const [showHidden, setShowHidden] = useState(false);
  const [popover, setPopover] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ key?: string; x: number; y: number } | null>(null);
  const [tooltip, setTooltip] = useState<Place | null>(null);
  const [preview, setPreview] = useState<number | null>(null);
  const [removedBookmark, setRemovedBookmark] = useState<import("../ipc/types").Bookmark | null>(null);
  const tipTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const tree = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const tipId = useId();
  const typeahead = useRef({ text: "", at: 0 });
  const sections = buildPlaces(state.places, state.connections, state.settings, state.home, showHidden);
  const entries = sections.flatMap(s => s.entries);
  const left = activePlace(entries, activeTabOf(state, 0).path), right = activePlace(entries, activeTabOf(state, 1).path);
  const collapsed = state.settings.sidebarCollapsed ?? [];
  const rows = sections.flatMap(s => [{ key: s.id, section: s.id, entry: null as Place | null }, ...(!collapsed.includes(s.id) || popover === s.id ? s.entries.map(entry => ({ key: entry.key, section: s.id, entry })) : [])]);
  const focusKey = rows.some(r => r.key === state.sidebarFocus) ? state.sidebarFocus! : rows[0]!.key;
  const label = (entry: Place) => entry.label ? tr(entry.label) : entry.name;
  const description = (entry: Place) => [label(entry), entry.state && tr(`sidebar.${entry.state}`), entry.space && tr("sidebar.space", { free: new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(entry.space.available / 1e9), total: new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(entry.space.total / 1e9) }), entry.path, entry.reason && tr(entry.reason)].filter(Boolean).join(" · ");
  function hideTooltip() { if (tipTimer.current) clearTimeout(tipTimer.current); setTooltip(null); }
  function showTooltip(entry: Place) {
    hideTooltip();
    tipTimer.current = setTimeout(() => {
      setTooltip(entry);
      if (entry.icon === "network" && entry.state === "mounted" && entry.path && !entry.space) {
        void controller.api.placesSpace(entry.path).then(space => setTooltip(current => current?.key === entry.key ? { ...current, space } : current)).catch(() => {});
      }
    }, 400);
  }
  useEffect(() => () => { if (tipTimer.current) clearTimeout(tipTimer.current); }, []);
  useEffect(() => { if (menu) menuRef.current?.querySelector<HTMLElement>("button")?.focus(); }, [menu]);
  useEffect(() => {
    if (mode === "hidden") { setMenu(null); setTooltip(null); }
  }, [mode]);
  function focus(key: string) {
    controller.setSidebarFocus(key);
    const node = [...(tree.current?.querySelectorAll<HTMLElement>("[data-place-key]") ?? [])].find(n => n.dataset.placeKey === key);
    node?.focus();
  }
  function context(key: string | undefined, x: number, y: number) { hideTooltip(); setMenu({ key, x: Math.min(x, window.innerWidth - 240), y: Math.min(y, window.innerHeight - 110) }); }
  function onKey(event: KeyboardEvent) {
    const index = rows.findIndex(r => r.key === focusKey), row = rows[index];
    if (!row) return;
    if (event.key === "ArrowDown") focus(rows[Math.min(rows.length - 1, index + 1)]!.key);
    else if (event.key === "ArrowUp") focus(rows[Math.max(0, index - 1)]!.key);
    else if (event.key === "Home") focus(rows[0]!.key);
    else if (event.key === "End") focus(rows.at(-1)!.key);
    else if (event.key === "ArrowLeft") { if (row.entry) focus(row.section); else controller.collapseSidebarSection(row.section, true); setPopover(null); }
    else if (event.key === "ArrowRight") { if (!row.entry) { controller.collapseSidebarSection(row.section, false); setPopover(null); } }
    else if (event.key === "Enter") {
      if (row.entry) void controller.openPlace(row.key, event.ctrlKey ? "other" : event.shiftKey ? "tab" : "active");
      else controller.collapseSidebarSection(row.section, !collapsed.includes(row.section));
    } else if (event.key === "Escape") { setPopover(null); controller.focusFileList(); }
    else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      const rect = (event.target as HTMLElement).getBoundingClientRect(); context(row.entry?.key, rect.left, rect.bottom);
    } else if (event.key === "Delete" && row.key.startsWith("bookmark:")) {
      const b = state.settings.bookmarks.find(b => `bookmark:${b.path}` === row.key);
      if (b) { setRemovedBookmark(b); controller.unpinBookmark(b.path); }
    } else if (!event.ctrlKey && !event.altKey && !event.metaKey && event.key.length === 1 && event.key !== " ") {
      const now = Date.now(); typeahead.current = { at: now, text: (now - typeahead.current.at < 800 ? typeahead.current.text : "") + event.key.toLocaleLowerCase() };
      const found = [...rows.slice(index + 1), ...rows.slice(0, index + 1)].find(r => (r.entry ? label(r.entry) : tr(sections.find(s => s.id === r.section)!.label)).toLocaleLowerCase().startsWith(typeahead.current.text));
      if (found) focus(found.key);
    } else return;
    event.preventDefault(); event.stopPropagation();
  }
  if (mode === "hidden") return null;
  const menuEntry = entries.find(e => e.key === menu?.key);
  function entryNode(entry: Place, inPopover = false) {
    const used = entry.space?.total ? 1 - entry.space.available / entry.space.total : null;
    return <div role="none" className="sidebar-entry-wrap" key={entry.key}>
      <button type="button" role="treeitem" aria-level={2} data-place-key={entry.key} tabIndex={entry.key === focusKey ? 0 : -1}
        aria-current={(state.active === 0 ? left : right) === entry.key ? "location" : undefined}
        aria-disabled={entry.state === "unmounted" || entry.disabled} aria-label={description(entry)} aria-describedby={tooltip?.key === entry.key ? tipId : undefined}
        className={`sidebar-entry ${entry.hidden ? "place-hidden" : ""} ${entry.state === "unmounted" || entry.disabled ? "unmounted" : ""}`}
        onFocus={() => { controller.setSidebarFocus(entry.key); showTooltip(entry); }} onBlur={hideTooltip} onMouseEnter={() => showTooltip(entry)} onMouseLeave={hideTooltip}
        onClick={event => void controller.openPlace(entry.key, event.ctrlKey ? "other" : "active")}
        onAuxClick={event => { if (event.button === 1) { event.preventDefault(); void controller.openPlace(entry.key, "tab"); } }}
        onContextMenu={event => { event.preventDefault(); context(entry.key, event.clientX, event.clientY); }}>
        <span className="place-icon" style={entry.color ? { color: entry.color } : undefined}><Icon name={entry.icon} />
          {mode === "icons" && !inPopover && !entry.color && <small className="place-badge">{entry.badge}</small>}
          {entry.state && <i className={`place-state ${entry.state}`} />}
        </span>
        {(mode === "full" || inPopover) && <><span className="place-name">{label(entry)}</span>{entry.hint && <kbd>Alt+{entry.hint}</kbd>}<span className="place-markers">{left === entry.key ? "L" : ""}{right === entry.key ? "R" : ""}</span></>}
        {used !== null && <span className={`place-space ${used > .9 ? "warning" : ""}`} aria-hidden="true"><i style={{ width: `${Math.max(0, Math.min(100, used * 100))}%` }} /></span>}
      </button>
    </div>;
  }
  return <nav className={`sidebar sidebar-${mode}`} aria-label={tr("sidebar.places")} style={{ width: mode === "icons" ? 44 : width }}>
    <div className="sidebar-tree" role="tree" aria-label={tr("sidebar.places")} ref={tree} onKeyDown={onKey}>
      {sections.map(section => {
        const closed = collapsed.includes(section.id), expanded = !closed || popover === section.id;
        return <div role="none" className="sidebar-section" key={section.id}>
          <button role="treeitem" type="button" aria-level={1} aria-expanded={expanded} aria-owns={expanded ? `${tipId}-${section.id}` : undefined} aria-label={tr(section.label)}
            data-place-key={section.id} tabIndex={focusKey === section.id ? 0 : -1} className="sidebar-heading"
            onFocus={() => controller.setSidebarFocus(section.id)} onClick={() => { if (mode === "icons" && closed) setPopover(popover === section.id ? null : section.id); else controller.collapseSidebarSection(section.id, !closed); }}
            onContextMenu={e => { e.preventDefault(); context(undefined, e.clientX, e.clientY); }}>
            {mode === "full" ? <><Icon name={expanded ? "down" : "right"} size={12} />{tr(section.label)}</> : closed ? "…" : <span className="rail-divider" />}
          </button>
          {expanded && <div role="group" id={`${tipId}-${section.id}`} className={mode === "icons" && closed ? "sidebar-popover" : undefined}>{section.entries.map(entry => entryNode(entry, mode === "icons" && closed))}</div>}
        </div>;
      })}
    </div>
    <button className="sidebar-new" onClick={() => controller.openDialog({ kind: "connections" })} aria-label={tr("header.manageConnections")} title={tr("header.manageConnections")}><Icon name="connect" />{mode === "full" && tr("header.manageConnections")}</button>
    <button className="sidebar-new" onClick={() => void controller.openConnectionEditor()} aria-label={tr("places.new")} title={tr("places.new")}><Icon name="plus" />{mode === "full" && tr("places.new")}</button>
    <button className="sidebar-expand" onClick={() => controller.toggleSidebarIcons()} aria-label={tr(mode === "icons" ? "sidebar.expand" : "sidebar.icons")} aria-expanded={mode === "full"}><Icon name={mode === "icons" ? "right" : "back"} />{mode === "full" && tr("sidebar.icons")}</button>
    <div role="separator" aria-label={tr("sidebar.width")} aria-orientation="vertical" aria-valuenow={mode === "icons" ? 44 : width} aria-valuemin={44} aria-valuemax={400} tabIndex={0} className="sidebar-resizer" style={preview === null ? undefined : { transform: `translateX(${preview - (mode === "icons" ? 44 : width)}px)` }}
      onDoubleClick={() => controller.toggleSidebarIcons()}
      onKeyDown={e => { if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); e.stopPropagation(); controller.setSidebarWidth((state.settings.sidebarWidth ?? 210) + (e.key === "ArrowRight" ? 10 : -10)); } }}
      onPointerDown={event => {
        event.preventDefault(); const target = event.currentTarget; target.setPointerCapture(event.pointerId);
        const start = event.clientX, initial = mode === "icons" ? 44 : width;
        const move = (e: PointerEvent) => setPreview(Math.max(44, Math.min(400, initial + e.clientX - start)));
        const finish = (e: PointerEvent) => { target.removeEventListener("pointermove", move); target.removeEventListener("pointerup", finish); target.removeEventListener("pointercancel", cancel); setPreview(null);
          const next = initial + e.clientX - start;
          if (next < (mode === "icons" ? 100 : 120)) controller.updateSettings(s => ({ ...s, sidebarMode: "icons" })); else controller.setSidebarWidth(next);
        };
        const cancel = () => { target.removeEventListener("pointermove", move); target.removeEventListener("pointerup", finish); target.removeEventListener("pointercancel", cancel); setPreview(null); };
        target.addEventListener("pointermove", move); target.addEventListener("pointerup", finish); target.addEventListener("pointercancel", cancel);
      }} />
    {tooltip && <div className="sidebar-tooltip" id={tipId} role="tooltip">{description(tooltip)}</div>}
    {menu && <div ref={menuRef} className="sidebar-menu" role="menu" style={{ left: menu.x, top: menu.y }} onBlur={e => { if (!e.currentTarget.contains(e.relatedTarget)) setMenu(null); }} onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); setMenu(null); focus(focusKey); } }}>
      {menuEntry && !menuEntry.key.startsWith("bookmark:") && <button role="menuitem" onClick={() => { controller.hidePlace(menuEntry.key, !menuEntry.hidden); setMenu(null); focus(menuEntry.key); }}>{tr(menuEntry.hidden ? "sidebar.showAgain" : "sidebar.hideEntry")}</button>}
      <button role="menuitemcheckbox" aria-checked={showHidden} onClick={() => { setShowHidden(!showHidden); setMenu(null); focus(focusKey); }}>{tr("sidebar.showHidden")}</button>
    </div>}
    {removedBookmark && <div role="status" className="sidebar-undo">{tr("sidebar.bookmarkRemoved")} <button onClick={() => { const b = removedBookmark; controller.updateSettings(s => ({ ...s, bookmarks: [...s.bookmarks.filter(v => v.path !== b.path), b] })); setRemovedBookmark(null); }}>{tr("sidebar.undo")}</button></div>}
    <span className="sr-only" role="status" aria-live="polite">{state.placesAnnouncement}</span>
  </nav>;
}
