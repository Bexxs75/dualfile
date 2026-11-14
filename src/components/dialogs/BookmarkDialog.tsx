import { useState } from "react";
import { useAppState, useController, useT } from "../../app/context";
import { accentPresets } from "../../themes/tokens";
import { DEFAULT_BOOKMARK_COLOR } from "../../state/bookmarks";
import { Modal } from "../Modal";

/** Lesezeichen bearbeiten: Name, Farbe (Vorgaben oder #rrggbb), lösen. */
export function BookmarkDialog({ path }: { path: string }) {
  const controller = useController();
  const tr = useT();
  const bookmark = useAppState((state) => state.settings.bookmarks.find((b) => b.path === path));
  const [name, setName] = useState(bookmark?.name ?? "");
  const [color, setColor] = useState(bookmark?.color ?? DEFAULT_BOOKMARK_COLOR);
  const [error, setError] = useState<string | null>(null);
  if (!bookmark) return null;

  function save(): void {
    const key = controller.editBookmark(path, { name, color });
    if (key) setError(tr(key));
    else controller.closeDialog();
  }

  return (
    <Modal
      title={tr("bookmarks.rename")}
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button
            type="button"
            className="tb warn"
            onClick={() => {
              controller.unpinBookmark(path);
              controller.closeDialog();
            }}
          >
            {tr("bookmarks.unpin")}
          </button>
          <button type="button" className="tb" onClick={() => controller.closeDialog()}>
            {tr("action.cancel")}
          </button>
          <button type="submit" form="bookmark-form" className="tb primary">
            {tr("action.ok")}
          </button>
        </>
      }
    >
      <form
        id="bookmark-form"
        className="form"
        onSubmit={(event) => {
          event.preventDefault();
          save();
        }}
      >
        <label className="fld">
          <span>{tr("dialog.properties.name")}</span>
          <input data-autofocus value={name} onChange={(event) => setName(event.target.value)} />
        </label>
        <div className="fld">
          <span>{tr("bookmarks.color")}</span>
          <div className="accent">
            <div className="swatches" role="group" aria-label={tr("bookmarks.color")}>
              {accentPresets.map((preset) => (
                <button
                  key={preset}
                  type="button"
                  className={color.toLowerCase() === preset ? "swatch on" : "swatch"}
                  style={{ background: preset }}
                  aria-label={preset}
                  aria-pressed={color.toLowerCase() === preset}
                  onClick={() => setColor(preset)}
                />
              ))}
            </div>
            <input className="hex" aria-label={tr("ui.settings.hex")} value={color} maxLength={7} spellCheck={false} onChange={(event) => setColor(event.target.value)} />
          </div>
        </div>
        {error ? (
          <p className="field-error" role="alert">
            {error}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}
