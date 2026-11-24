import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Icon, type IconName } from "./Icon";

export type HeaderMenuAction = { label: string; icon?: IconName; ariaLabel?: string; action: () => void; hint?: string; disabled?: boolean; reason?: string; current?: boolean; connected?: boolean; };
/** A row with `submenu` opens a side menu instead of running an action. */
export type HeaderMenuParent = { label: string; ariaLabel?: string; detail?: string; connected?: boolean; submenu: HeaderMenuAction[] };
export type HeaderMenuItem = HeaderMenuAction | HeaderMenuParent | { kind: "heading"; label: string; connected?: boolean; } | { kind: "separator" };
/** Native buttons activate with Enter/Space; menu navigation stays local. */
export function HeaderMenu({ label, title, children, items, className = "", portal = false }: { label: string; title?: string; children: ReactNode; items: HeaderMenuItem[]; className?: string; portal?: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const last = useRef(false);
  const id = useId();
  const [sub, setSub] = useState<number | null>(null);
  const subMenu = useRef<HTMLDivElement>(null);
  const subFocus = useRef(false);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const close = () => { setOpen(false); setSub(null); trigger.current?.focus(); };
  const rowOf = (index: number) => menu.current?.querySelector<HTMLButtonElement>(`[data-row="${index}"]`) ?? null;
  const closeSub = (focusRow: boolean) => { const row = sub === null ? null : rowOf(sub); setSub(null); if (focusRow) row?.focus(); };
  // The submenu is fixed-positioned outside the scrolling main menu, so it is placed from live rects.
  const placeSub = () => {
    const el = subMenu.current, row = sub === null ? null : rowOf(sub), host = menu.current;
    if (!el || !row || !host) return;
    const r = row.getBoundingClientRect(), m = host.getBoundingClientRect(), w = el.offsetWidth, h = el.offsetHeight;
    let left = m.right + 2;
    if (left + w > window.innerWidth - 4) left = Math.max(4, m.left - w - 2);
    const top = Math.max(4, Math.min(r.top - 5, window.innerHeight - h - 4));
    el.style.left = `${left}px`; el.style.top = `${top}px`;
  };
  useLayoutEffect(placeSub, [sub, open]);
  useEffect(() => {
    if (sub === null || !subFocus.current) return;
    subFocus.current = false;
    subMenu.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [sub]);
  useEffect(() => {
    if (!open) return;
    const rect = trigger.current?.getBoundingClientRect();
    if (rect) setPosition({ top: rect.bottom + 4, left: Math.max(4, Math.min(rect.left, window.innerWidth - 300)) });
    const buttons = menu.current?.querySelectorAll<HTMLButtonElement>("button");
    buttons?.[last.current ? buttons.length - 1 : 0]?.focus();
    const outside = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node) && !menu.current?.contains(e.target as Node) && !subMenu.current?.contains(e.target as Node)) setOpen(false); };
    const reposition = () => { setSub(null); if (portal) close(); };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", reposition);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", reposition);
    };
  }, [open]);
  useEffect(() => { if (!open) setSub(null); }, [open]);
  const renderMenu = (node: ReactNode) => portal ? createPortal(node, document.body) : node;
  return <div className={`header-menu ${className}`} ref={root} onBlur={e => { if (!e.currentTarget.contains(e.relatedTarget) && !menu.current?.contains(e.relatedTarget) && !subMenu.current?.contains(e.relatedTarget)) setOpen(false); }}>
    <button ref={trigger} type="button" className="header-chip" aria-label={label} title={open ? undefined : title} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
      onClick={() => { last.current = false; setOpen(!open); }} onKeyDown={e => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); e.stopPropagation(); last.current = e.key === "ArrowUp"; setOpen(true); }
      }}>{children}<Icon name="down" /></button>
    {open && renderMenu(<div ref={menu} style={portal ? { position: "fixed", ...position, zIndex: 35, maxHeight: Math.max(80, window.innerHeight - position.top - 8) } : undefined} id={id} role="menu" aria-label={label} className="header-dropdown" onScroll={placeSub} onKeyDown={e => {
      const buttons = [...(menu.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) setSub(null);
      const row = items.filter(i => !("kind" in i))[index];
      if (e.key === "Escape") close();
      else if (e.key === "Tab") { setOpen(false); return; }
      else if (e.key === "ArrowRight" && row && "submenu" in row) { subFocus.current = true; setSub(index); }
      else if (e.key === "ArrowDown") buttons[(index + 1) % buttons.length]?.focus();
      else if (e.key === "ArrowUp") buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
      else if (e.key === "Home") buttons[0]?.focus();
      else if (e.key === "End") buttons.at(-1)?.focus();
      else return;
      e.preventDefault(); e.stopPropagation();
    }}>
      {(() => { let row = -1; return items.map((item, i) => {
        if ("kind" in item && item.kind === "separator") return <div key={i} role="separator" className="header-menu-separator" />;
        if ("kind" in item) return <div key={i} className="header-menu-heading">
          {item.connected !== undefined && <span aria-hidden="true" className={`connection-dot ${item.connected ? "connected" : "disconnected"}`} />}<span>{item.label}</span>
        </div>;
        row += 1;
        if ("submenu" in item) {
          const mine = row, expanded = sub === mine;
          return <button key={i} type="button" role="menuitem" data-row={mine} tabIndex={-1} aria-label={item.ariaLabel ?? item.label} aria-haspopup="menu" aria-expanded={expanded} aria-controls={expanded ? `${id}-sub` : undefined}
            onClick={() => { if (expanded) closeSub(true); else { subFocus.current = true; setSub(mine); } }}>
            {item.connected !== undefined && <span aria-hidden="true" className={`connection-dot ${item.connected ? "connected" : "disconnected"}`} />}<span className="menu-name">{item.label}</span>{item.detail && <small className="menu-hint menu-detail">{item.detail}</small>}<span aria-hidden="true" className="menu-chevron">›</span>
          </button>;
        }
        return <button type="button" role="menuitem" data-row={row} aria-disabled={item.disabled || undefined} aria-description={item.disabled ? item.reason : undefined} aria-current={item.current ? "location" : undefined} tabIndex={-1} aria-label={item.ariaLabel ?? item.label} key={i} onClick={() => { if (item.disabled) return; close(); item.action(); }}>{item.icon && <Icon name={item.icon} />}<span>{item.label}{item.disabled && item.reason && <small className="menu-reason">{item.reason}</small>}</span>{item.connected !== undefined && <span aria-hidden="true" className={`connection-dot ${item.connected ? "connected" : "disconnected"}`} />}{item.hint && <small className="menu-hint">{item.hint}</small>}{item.current && <Icon name="check" />}</button>;
      }); })()}
    </div>)}
    {open && sub !== null && (() => {
      const parent = items.filter(i => !("kind" in i))[sub];
      if (!parent || !("submenu" in parent)) return null;
      return createPortal(<div ref={subMenu} id={`${id}-sub`} role="menu" aria-label={parent.ariaLabel ?? parent.label} className="header-dropdown header-submenu" style={{ position: "fixed", right: "auto", zIndex: 36, maxHeight: window.innerHeight - 8 }} onKeyDown={e => {
        const buttons = [...(subMenu.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        if (e.key === "Escape" || e.key === "ArrowLeft") closeSub(true);
        else if (e.key === "Tab") { setOpen(false); return; }
        else if (e.key === "ArrowDown") buttons[(index + 1) % buttons.length]?.focus();
        else if (e.key === "ArrowUp") buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
        else if (e.key === "Home") buttons[0]?.focus();
        else if (e.key === "End") buttons.at(-1)?.focus();
        else return;
        e.preventDefault(); e.stopPropagation();
      }}>
        {parent.submenu.map((item, i) => <button key={i} type="button" role="menuitem" tabIndex={-1} aria-disabled={item.disabled || undefined} aria-description={item.disabled ? item.reason : undefined} aria-label={item.ariaLabel ?? item.label} onClick={() => { if (item.disabled) return; close(); item.action(); }}>{item.icon && <Icon name={item.icon} />}<span>{item.label}{item.disabled && item.reason && <small className="menu-reason">{item.reason}</small>}</span></button>)}
      </div>, document.body);
    })()}
  </div>;
}
