import { useEffect, useState, type CSSProperties } from "react";
import { useAppState, useController, useT } from "../app/context";
import type { WinIdx } from "../app/state";
import type { Drive } from "../ipc/types";
import { DEFAULT_BOOKMARK_COLOR, isValidColor } from "../state/bookmarks";
import { Icon } from "./Icon";

/** Schnellzugriff-Leiste je Pane: Lesezeichen, Home und lokale Laufwerke; Klick öffnet den Ort in DIESER Pane. */
export function QuickAccessBar({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const enabled = useAppState(state => state.settings.showQuickAccess === true);
  const home = useAppState(state => state.home);
  const bookmarks = useAppState(state => state.settings.bookmarks);
  const [drives, setDrives] = useState<Drive[]>([]);
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    void controller.api.localDrives().then(d => { if (alive) setDrives(d); }).catch(() => {});
    return () => { alive = false; };
  }, [controller, enabled]);
  const ordered = [...bookmarks].sort((a, b) => a.order - b.order);
  if (!enabled) return null;
  return (
    <nav className="qa-bar" aria-label={tr("quickAccess.label")}>
      {ordered.map(bookmark => (
        <button
          key={`b:${bookmark.path}`}
          type="button"
          className="qa-item qa-bm"
          style={{ "--c": isValidColor(bookmark.color) ? bookmark.color : DEFAULT_BOOKMARK_COLOR } as CSSProperties}
          title={bookmark.path}
          onClick={() => void controller.navigatePlace(w, bookmark.path)}
        >
          <Icon name="pin" /><span>{bookmark.name}</span>
        </button>
      ))}
      <button type="button" className="qa-item" title={home} onClick={() => void controller.navigatePlace(w, home)}>
        <Icon name="home" /><span>{tr("places.home")}</span>
      </button>
      {drives.map(drive => (
        <button
          key={`d:${drive.path}`}
          type="button"
          className="qa-item"
          title={drive.path}
          onClick={() => void controller.navigatePlace(w, drive.path)}
        >
          <Icon name="disk" /><span>{drive.name}</span>
        </button>
      ))}
    </nav>
  );
}
