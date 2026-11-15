import { useEffect, useState, type CSSProperties } from "react";
import { useAppState, useController, useT } from "../app/context";
import type { Drive } from "../ipc/types";
import { DEFAULT_BOOKMARK_COLOR, isValidColor } from "../state/bookmarks";
import { Icon } from "./Icon";

/** Schnellzugriff-Leiste über den Panes: Lesezeichen und Laufwerke; Klick öffnet den Ort in der aktiven Pane. */
export function QuickAccessBar() {
  const controller = useController();
  const tr = useT();
  const enabled = useAppState(state => state.settings.showQuickAccess === true);
  const active = useAppState(state => state.active);
  const bookmarks = useAppState(state => state.settings.bookmarks);
  const [drives, setDrives] = useState<Drive[]>([]);
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    void controller.api.localDrives().then(d => { if (alive) setDrives(d); }).catch(() => {});
    return () => { alive = false; };
  }, [controller, enabled]);
  const ordered = [...bookmarks].sort((a, b) => a.order - b.order);
  if (!enabled || (!ordered.length && !drives.length)) return null;
  return (
    <nav className="qa-bar" aria-label={tr("quickAccess.label")}>
      {ordered.map(bookmark => (
        <button
          key={`b:${bookmark.path}`}
          type="button"
          className="qa-item qa-bm"
          style={{ "--c": isValidColor(bookmark.color) ? bookmark.color : DEFAULT_BOOKMARK_COLOR } as CSSProperties}
          title={bookmark.path}
          onClick={() => void controller.navigatePlace(active, bookmark.path)}
        >
          <Icon name="pin" /><span>{bookmark.name}</span>
        </button>
      ))}
      {drives.map(drive => (
        <button
          key={`d:${drive.path}`}
          type="button"
          className="qa-item"
          title={drive.path}
          onClick={() => void controller.navigatePlace(active, drive.path)}
        >
          <Icon name="disk" /><span>{drive.name}</span>
        </button>
      ))}
    </nav>
  );
}
