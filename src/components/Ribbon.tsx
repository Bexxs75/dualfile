import { useState, type CSSProperties } from "react";
import { useAppState, useController, useT } from "../app/context";
import { Icon } from "./Icon";
import { DEFAULT_BOOKMARK_COLOR, isValidColor } from "../state/bookmarks";

/** Lesezeichenband: angeheftete Ordner, Klick öffnet sie im aktiven Fenster; Spiegeln-Schalter am Ende. */
export function Ribbon() {
  const controller = useController();
  const tr = useT();
  const bookmarks = useAppState((state) => state.settings.bookmarks);
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const ordered = [...bookmarks].sort((a, b) => a.order - b.order);
  const noFocus = (event: { preventDefault(): void }) => event.preventDefault();
  if (!ordered.length) return null;
  return (
    <div className="ribbon" role="toolbar" aria-label={tr("bookmarks.title")}>
      {ordered.map((bookmark, index) => (
        <button
          key={bookmark.path}
          type="button"
          className="bm"
          style={{ "--c": isValidColor(bookmark.color) ? bookmark.color : DEFAULT_BOOKMARK_COLOR } as CSSProperties}
          title={`${bookmark.path} – ${tr("bookmarks.dragHint")}`}
          draggable
          onMouseDown={noFocus}
          onClick={() => void controller.openBookmark(bookmark.path)}
          onContextMenu={(event) => {
            event.preventDefault();
            controller.openDialog({ kind: "bookmark", path: bookmark.path });
          }}
          onDragStart={() => setDragFrom(index)}
          onDragOver={(event) => {
            if (dragFrom !== null) event.preventDefault();
          }}
          onDrop={(event) => {
            event.preventDefault();
            if (dragFrom !== null && dragFrom !== index) controller.moveBookmark(dragFrom, index);
            setDragFrom(null);
          }}
          onDragEnd={() => setDragFrom(null)}
        >
          <Icon name="dir" />{bookmark.name}
          {index < 9 ? <kbd>{tr("bookmarks.shortcut", { number: index + 1 })}</kbd> : null}
        </button>
      ))}
    </div>
  );
}
