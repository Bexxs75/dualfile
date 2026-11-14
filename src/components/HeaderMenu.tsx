import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Icon, type IconName } from "./Icon";

export type HeaderMenuItem = { label: string; icon?: IconName; ariaLabel?: string; action: () => void; hint?: string; current?: boolean; connected?: boolean; } | { kind: "heading"; label: string; connected?: boolean; };
/** Native buttons activate with Enter/Space; menu navigation stays local. */
export function HeaderMenu({ label, title, children, items, className = "", portal = false }: { label: string; title?: string; children: ReactNode; items: HeaderMenuItem[]; className?: string; portal?: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const last = useRef(false);
  const id = useId();
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const close = () => { setOpen(false); trigger.current?.focus(); };
  useEffect(() => {
    if (!open) return;
    const rect = trigger.current?.getBoundingClientRect();
    if (rect) setPosition({ top: rect.bottom + 4, left: Math.max(4, Math.min(rect.left, window.innerWidth - 300)) });
    const buttons = menu.current?.querySelectorAll<HTMLButtonElement>("button");
    buttons?.[last.current ? buttons.length - 1 : 0]?.focus();
    const outside = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node) && !menu.current?.contains(e.target as Node)) setOpen(false); };
    const reposition = () => { if (portal) close(); };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", reposition);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", reposition);
    };
  }, [open]);
  const renderMenu = (node: ReactNode) => portal ? createPortal(node, document.body) : node;
  return <div className={`header-menu ${className}`} ref={root} onBlur={e => { if (!e.currentTarget.contains(e.relatedTarget) && !menu.current?.contains(e.relatedTarget)) setOpen(false); }}>
    <button ref={trigger} type="button" className="header-chip" aria-label={label} title={open ? undefined : title} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
      onClick={() => { last.current = false; setOpen(!open); }} onKeyDown={e => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); e.stopPropagation(); last.current = e.key === "ArrowUp"; setOpen(true); }
      }}>{children}<Icon name="down" /></button>
    {open && renderMenu(<div ref={menu} style={portal ? { position: "fixed", ...position, zIndex: 35, maxHeight: Math.max(80, window.innerHeight - position.top - 8) } : undefined} id={id} role="menu" aria-label={label} className="header-dropdown" onKeyDown={e => {
      const buttons = [...(menu.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "Escape") close();
      else if (e.key === "Tab") { setOpen(false); return; }
      else if (e.key === "ArrowDown") buttons[(index + 1) % buttons.length]?.focus();
      else if (e.key === "ArrowUp") buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
      else if (e.key === "Home") buttons[0]?.focus();
      else if (e.key === "End") buttons.at(-1)?.focus();
      else return;
      e.preventDefault(); e.stopPropagation();
    }}>
      {items.map((item, i) => "kind" in item ? <div key={i} className="header-menu-heading">
        {item.connected !== undefined && <span aria-hidden="true" className={`connection-dot ${item.connected ? "connected" : "disconnected"}`} />}<span>{item.label}</span>
      </div> : <button type="button" role="menuitem" aria-current={item.current ? "location" : undefined} tabIndex={-1} aria-label={item.ariaLabel} key={i} onClick={() => { close(); item.action(); }}>{item.icon && <Icon name={item.icon} />}<span>{item.label}</span>{item.connected !== undefined && <span aria-hidden="true" className={`connection-dot ${item.connected ? "connected" : "disconnected"}`} />}{item.hint && <small className="menu-hint">{item.hint}</small>}{item.current && <Icon name="check" />}</button>)}
    </div>)}
  </div>;
}
