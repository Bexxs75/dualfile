import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useController, useLanguage, useT } from "../../app/context";
import { formatBytes } from "../../i18n";
import type { FileInfo } from "../../ipc/types";
import { HexView } from "./HexView";
import { MediaPane } from "./MediaPane";
import { TextView, type TextPosition } from "./TextView";
import {
  ENCODINGS,
  ENCODING_LABEL,
  autoMode,
  canShowMedia,
  detectBom,
  readRange,
  type TextEncodingId,
  type ViewMode,
} from "./lib";

/** Einstellungen, die beim Blättern von Datei zu Datei erhalten bleiben. */
export interface ViewPrefs {
  wrap: boolean;
  lineNumbers: boolean;
  fontSize: number;
}

export const defaultPrefs: ViewPrefs = { wrap: false, lineNumbers: true, fontSize: 13 };
const MIN_FONT = 9;
const MAX_FONT = 28;

type Loaded = { phase: "loading" } | { phase: "error"; message: string } | { phase: "ready"; info: FileInfo; bom: TextEncodingId | null };

interface FileViewerProps {
  path: string;
  name: string;
  prefs: ViewPrefs;
  onPrefs(prefs: ViewPrefs): void;
  /** Im Lister soll der Fokus nach einem Dateiwechsel in der Ansicht bleiben. */
  focusOnMount?: boolean;
}

/** Gemeinsamer Inhalt von Lister und Schnellansicht: Kopfleiste, Ansicht nach Art der Datei, Statuszeile. */
export function FileViewer({ path, name, prefs, onPrefs, focusOnMount = false }: FileViewerProps) {
  const { api, describeError } = useController();
  const tr = useT();
  const language = useLanguage();
  const [loaded, setLoaded] = useState<Loaded>({ phase: "loading" });
  const [manualMode, setManualMode] = useState<ViewMode | null>(null);
  const [manualEncoding, setManualEncoding] = useState<TextEncodingId | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [position, setPosition] = useState<{ percent: number; line: number | null; offset: number | null }>({ percent: 0, line: null, offset: null });

  // Neue Datei: alles zurücksetzen, Art und Byte-Order-Mark lesen.
  useEffect(() => {
    let cancelled = false;
    setLoaded({ phase: "loading" });
    setManualMode(null);
    setManualEncoding(null);
    setNotice(null);
    setPosition({ percent: 0, line: null, offset: null });
    void (async () => {
      try {
        const info = await api.fileInfo(path);
        const head = info.kind === "text" && info.size > 0 ? await readRange(api, path, 0, 4) : new Uint8Array(0);
        if (!cancelled) setLoaded({ phase: "ready", info, bom: detectBom(head)?.encoding ?? null });
      } catch (error) {
        if (!cancelled) setLoaded({ phase: "error", message: describeError(error, { path }) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [api, path, describeError]);

  const root = useRef<HTMLDivElement>(null);
  const readyMode = loaded.phase === "ready" ? (manualMode ?? autoMode(loaded.info)) : null;
  // Im Lister bleibt der Fokus nach Dateiwechsel oder Moduswechsel in der Ansicht (das alte Element ist weg).
  useEffect(() => {
    if (!focusOnMount || readyMode === null) return;
    const active = document.activeElement;
    if (active === document.body || (active instanceof HTMLElement && active.hasAttribute("data-lister-root"))) {
      root.current?.querySelector<HTMLElement>("[data-viewer-focus]")?.focus();
    }
  }, [focusOnMount, readyMode, loaded.phase]);

  const onText = useCallback((p: TextPosition) => setPosition({ percent: p.percent, line: p.line, offset: null }), []);
  const onHex = useCallback((p: { percent: number; offset: number }) => setPosition({ percent: p.percent, line: null, offset: p.offset }), []);

  if (loaded.phase === "loading") return <p className="pane-msg" role="status">{tr("lister.loading")}</p>;
  if (loaded.phase === "error") {
    return (
      <p className="pane-msg error" role="alert">
        {tr("lister.error", { reason: loaded.message })}
      </p>
    );
  }

  const { info, bom } = loaded;
  const mode = manualMode ?? autoMode(info);
  const encoding = manualEncoding ?? bom ?? "utf-8";

  function choose(next: ViewMode): void {
    if (next === "media" && !canShowMedia(info)) {
      setNotice(tr("lister.mode.mediaUnavailable"));
      return;
    }
    setNotice(null);
    setManualMode(next);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const target = event.target;
    if (event.ctrlKey && !event.altKey && !event.metaKey && (event.key.toLowerCase() === "f" || event.key.toLowerCase() === "g")) {
      // Suchen/Springen gehört der Ansicht: liegt der Fokus in der Kopfleiste, dorthin umleiten.
      const view = root.current?.querySelector<HTMLElement>("[data-viewer-focus]");
      const area = view?.closest(".lv");
      if (view && area && !(target instanceof Node && area.contains(target))) {
        event.preventDefault();
        event.stopPropagation();
        view.focus();
        view.dispatchEvent(new KeyboardEvent("keydown", { key: event.key, ctrlKey: true, bubbles: true, cancelable: true }));
      }
      return;
    }
    if (target instanceof HTMLElement && (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) || target.isContentEditable)) return;
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    const next = ({ "1": "text", "2": "hex", "3": "media" } as const)[event.key as "1" | "2" | "3"];
    if (!next) return;
    event.preventDefault();
    event.stopPropagation();
    choose(next);
  }

  const modes: { id: ViewMode; label: string; key: string }[] = [
    { id: "text", label: tr("lister.mode.text"), key: "1" },
    { id: "hex", label: tr("lister.mode.hex"), key: "2" },
    { id: "media", label: tr("lister.mode.media"), key: "3" },
  ];

  return (
    <div className="lv-viewer" ref={root} onKeyDown={onKeyDown}>
      <div className="lv-bar" role="toolbar" aria-label={tr("lister.modes")}>
        <div className="seg">
          {modes.map((entry) => (
            <button key={entry.id} type="button" className={mode === entry.id ? "on" : undefined} aria-pressed={mode === entry.id} onClick={() => choose(entry.id)}>
              {entry.label} <kbd>{entry.key}</kbd>
            </button>
          ))}
        </div>
        {mode === "text" ? (
          <>
            <label className="lv-sel">
              {tr("lister.encoding")}
              <select value={encoding} onChange={(event) => setManualEncoding(event.target.value as TextEncodingId)}>
                {ENCODINGS.map((id) => (
                  <option key={id} value={id}>
                    {ENCODING_LABEL[id]}
                  </option>
                ))}
              </select>
            </label>
            <label className="check">
              <input type="checkbox" checked={prefs.wrap} onChange={(event) => onPrefs({ ...prefs, wrap: event.target.checked })} />
              {tr("lister.wrap")}
            </label>
            <label className="check">
              <input type="checkbox" checked={prefs.lineNumbers} onChange={(event) => onPrefs({ ...prefs, lineNumbers: event.target.checked })} />
              {tr("lister.lineNumbers")}
            </label>
          </>
        ) : null}
        {mode !== "media" ? (
          <span className="lv-font">
            <button type="button" className="tb small" aria-label={tr("lister.fontSmaller")} onClick={() => onPrefs({ ...prefs, fontSize: Math.max(MIN_FONT, prefs.fontSize - 1) })}>
              A−
            </button>
            <button type="button" className="tb small" aria-label={tr("lister.fontLarger")} onClick={() => onPrefs({ ...prefs, fontSize: Math.min(MAX_FONT, prefs.fontSize + 1) })}>
              A+
            </button>
          </span>
        ) : null}
        {notice ? (
          <span className="lv-msg" role="status">
            {notice}
          </span>
        ) : null}
      </div>
      <div className="lv-main">
        {mode === "text" ? (
          <TextView
            path={path}
            size={info.size}
            encoding={encoding}
            wrap={prefs.wrap}
            lineNumbers={prefs.lineNumbers}
            fontSize={prefs.fontSize}
           
            onPosition={onText}
          />
        ) : null}
        {mode === "hex" ? <HexView path={path} size={info.size} fontSize={prefs.fontSize} onPosition={onHex} /> : null}
        {mode === "media" ? <MediaPane path={path} name={name} info={info} /> : null}
      </div>
      <div className="lv-status" role="status" aria-label={tr("lister.status")}>
        {mode === "text" ? <span>{ENCODING_LABEL[encoding]}</span> : null}
        <span>{tr("lister.status.size", { size: formatBytes(info.size, language) })}</span>
        {mode !== "media" ? <span>{tr("lister.status.percent", { percent: position.percent })}</span> : null}
        {mode === "text" && position.line !== null ? <span>{tr("lister.status.line", { line: position.line })}</span> : null}
        {mode === "hex" && position.offset !== null ? <span>{tr("lister.status.offset", { offset: `0x${position.offset.toString(16)}` })}</span> : null}
        <span className="grow">{info.mimeGuess}</span>
        {mode !== "media" ? <span>{tr("lister.textHint")}</span> : null}
      </div>
    </div>
  );
}
