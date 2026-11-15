import { useEffect, useRef, useState, type ReactNode } from "react";
import { useLanguage, useT } from "../app/context";
import { Keycap, type KeycapLabel } from "./Keycap";

const SHOW_DELAY_MS = 350;

/** Backquote key legend. Approximated via the UI language (de -> ^/°), as the real keyboard layout is not readable. */
export function useBackquoteKeycap(): KeycapLabel {
  return useLanguage() === "de" ? { top: "^", bot: "°" } : "`";
}

/** Wraps a control and shows a keycap hover card on hover/focus. */
export function ShortcutHint({ title, keys, position, children }: { title: string; keys: KeycapLabel[]; position?: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const show = () => { window.clearTimeout(timer.current); timer.current = window.setTimeout(() => setOpen(true), SHOW_DELAY_MS); };
  const hide = () => { window.clearTimeout(timer.current); setOpen(false); };
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return <span className="hint-wrap" onMouseEnter={show} onMouseLeave={hide} onFocus={show} onBlur={hide} onKeyDown={e => { if (e.key === "Escape") hide(); }}>
    {children}
    <span className="hovercard" role="tooltip" hidden={!open}>
      <span className="hc-title">{title}</span>
      <span className="hc-row">
        {keys.map((k, i) => <span key={i} className="hc-key">{i > 0 && <span className="plus">+</span>}<Keycap label={k} />
        </span>)}
        {position && <span className="hc-hint">{position}</span>}
      </span>
    </span>
  </span>;
}

/** Hover card for the terminal toggle (Ctrl + Backquote). */
export function TerminalShortcutHint({ children }: { children: ReactNode }) {
  const tr = useT();
  const bq = useBackquoteKeycap();
  return <ShortcutHint title={tr("hint.terminal.title")} keys={[tr("key.ctrl"), bq]} position={tr("hint.terminal.position")}>{children}</ShortcutHint>;
}
