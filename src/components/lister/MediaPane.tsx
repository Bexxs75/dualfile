import { useState, type KeyboardEvent } from "react";
import { useController, useLanguage, useT } from "../../app/context";
import { formatBytes } from "../../i18n";
import type { FileInfo } from "../../ipc/types";
import { isImageInfo, mediaLimit } from "./lib";
import { useBlobUrl } from "./useBlobUrl";

interface MediaProps {
  path: string;
  name: string;
  info: FileInfo;
}

const ZOOM_STEPS = [0.1, 0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4, 8];

/** Bildansicht: Blob-URL in `<img>` (auch SVG nur als Bild, nie als Dokument). Einpassen, 100 %, Zoomen, Drehen. */
function ImageView({ path, name, info }: MediaProps) {
  const tr = useT();
  const { state } = useBlobUrl(path, info, true);
  const [zoom, setZoom] = useState<"fit" | number>("fit");
  const [turns, setTurns] = useState(0);
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  const [broken, setBroken] = useState(false);

  function step(direction: 1 | -1): void {
    const current = zoom === "fit" ? 1 : zoom;
    const next = direction === 1 ? ZOOM_STEPS.find((z) => z > current + 1e-6) : [...ZOOM_STEPS].reverse().find((z) => z < current - 1e-6);
    if (next !== undefined) setZoom(next);
  }
  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    const handled = { "+": () => step(1), "-": () => step(-1), "0": () => setZoom(1), f: () => setZoom("fit"), r: () => setTurns((t) => (t + 1) % 4) }[event.key];
    if (handled) {
      event.preventDefault();
      handled();
    }
  }

  const style =
    zoom === "fit"
      ? { maxWidth: "100%", maxHeight: "100%" }
      : { width: natural ? natural.w * zoom : undefined, maxWidth: "none", maxHeight: "none" };
  return (
    <div className="lv" onKeyDown={onKeyDown}>
      <div className="lv-tools" role="toolbar" aria-label={tr("lister.mode.media")}>
        <button type="button" className={zoom === "fit" ? "tb small on" : "tb small"} onClick={() => setZoom("fit")}>
          {tr("lister.image.fit")}
        </button>
        <button type="button" className={zoom === 1 ? "tb small on" : "tb small"} onClick={() => setZoom(1)}>
          {tr("lister.image.actual")}
        </button>
        <button type="button" className="tb small" aria-label={tr("lister.image.zoomOut")} onClick={() => step(-1)}>
          −
        </button>
        <button type="button" className="tb small" aria-label={tr("lister.image.zoomIn")} onClick={() => step(1)}>
          +
        </button>
        <button type="button" className="tb small" onClick={() => setTurns((t) => (t + 1) % 4)}>
          ⟳ {tr("lister.image.rotate")}
        </button>
        {natural ? <span className="hint">{tr("lister.image.dimensions", { width: natural.w, height: natural.h })}</span> : null}
        {zoom !== "fit" ? <span className="hint">{tr("lister.image.zoom", { percent: Math.round(zoom * 100) })}</span> : null}
      </div>
      <div className="lv-scroll lv-image" tabIndex={0} data-viewer-focus aria-label={tr("lister.image.alt", { name })}>
        {state.phase === "loading" || state.phase === "idle" ? <p className="pane-msg">{tr("lister.media.loading", { name })}</p> : null}
        {state.phase === "error" ? (
          <p className="pane-msg error" role="alert">
            {tr("lister.error", { reason: state.message })}
          </p>
        ) : null}
        {broken ? (
          <p className="pane-msg error" role="alert">
            {tr("lister.image.failed")}
          </p>
        ) : null}
        {state.phase === "ready" && !broken ? (
          <img
            src={state.url}
            alt={tr("lister.image.alt", { name })}
            draggable={false}
            style={{ ...style, transform: turns ? `rotate(${turns * 90}deg)` : undefined }}
            onLoad={(event) => setNatural({ w: event.currentTarget.naturalWidth, h: event.currentTarget.naturalHeight })}
            onError={() => setBroken(true)}
          />
        ) : null}
      </div>
    </div>
  );
}

/** Audio/Video: wird erst auf Klick geladen und spielt nie von selbst (kein Autoplay). */
function AvView({ path, name, info }: MediaProps) {
  const tr = useT();
  const [wanted, setWanted] = useState(false);
  const { state } = useBlobUrl(path, info, wanted);
  const video = info.kind === "video" || info.mimeGuess.startsWith("video/");
  return (
    <div className="lv-scroll lv-av" data-viewer-focus tabIndex={0} aria-label={tr(video ? "lister.media.video" : "lister.media.audio")}>
      {state.phase === "idle" && !wanted ? (
        <button type="button" className="tb primary" data-viewer-focus onClick={() => setWanted(true)}>
          {tr("lister.media.load")}
        </button>
      ) : null}
      {state.phase === "loading" ? <p className="pane-msg">{tr("lister.media.loading", { name })}</p> : null}
      {state.phase === "error" ? (
        <p className="pane-msg error" role="alert">
          {tr("lister.error", { reason: state.message })}
        </p>
      ) : null}
      {state.phase === "ready" && video ? <video src={state.url} controls preload="metadata" aria-label={name} /> : null}
      {state.phase === "ready" && !video ? <audio src={state.url} controls preload="metadata" aria-label={name} /> : null}
    </div>
  );
}

/** Bild/Medien-Ansicht mit Obergrenze: darüber nur Meldung und Knopf zum externen Öffnen. */
export function MediaPane({ path, name, info }: MediaProps) {
  const tr = useT();
  const language = useLanguage();
  const { openExternalPath } = useController();
  const limit = mediaLimit(info);
  if (info.size > limit) {
    return (
      <div className="lv-scroll lv-av">
        <p className="pane-msg" role="status">
          {tr("lister.media.tooBig", { size: formatBytes(info.size, language), limit: formatBytes(limit, language) })}
        </p>
        <button type="button" className="tb primary" data-viewer-focus onClick={() => void openExternalPath(path)}>
          {tr("lister.media.openExternal")}
        </button>
      </div>
    );
  }
  return isImageInfo(info) ? <ImageView path={path} name={name} info={info} /> : <AvView path={path} name={name} info={info} />;
}
