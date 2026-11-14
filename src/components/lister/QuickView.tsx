import { useEffect, useState } from "react";
import { useAppState, useT } from "../../app/context";
import { focusedEntry, focusedFilePath, type WinIdx } from "../../app/state";
import { baseName } from "../../state/paths";
import { FileViewer, defaultPrefs, type ViewPrefs } from "./FileViewer";

/** Wartezeit, bevor die Vorschau dem Fokus der Liste folgt (Pfeiltasten sollen nicht jede Datei laden). */
export const QUICK_VIEW_DELAY_MS = 150;

function useDebounced<T>(value: T, ms: number): T {
  const [shown, setShown] = useState(value);
  useEffect(() => {
    if (Object.is(shown, value)) return;
    const timer = setTimeout(() => setShown(value), ms);
    return () => clearTimeout(timer);
  }, [value, shown, ms]);
  return shown;
}

/**
 * Schnellansicht (Strg+Q): Vorschau der fokussierten Datei des Fensters `source` anstelle des anderen
 * Fensters. Folgt dem Fokus, entprellt. Solange der Lister offen ist, ruht sie (ein Blob zugleich).
 */
export function QuickView({ source }: { source: WinIdx }) {
  const tr = useT();
  const path = useAppState((state) => focusedFilePath(state, source));
  const kind = useAppState((state) => focusedEntry(state, source)?.kind ?? null);
  const listerOpen = useAppState((state) => state.lister !== null);
  const [prefs, setPrefs] = useState<ViewPrefs>(defaultPrefs);
  const shown = useDebounced(path, QUICK_VIEW_DELAY_MS);
  const current = path === null ? null : shown;

  let body;
  if (listerOpen) body = <p className="pane-msg">{tr("lister.quick.paused")}</p>;
  else if (kind === "dir") body = <p className="pane-msg">{tr("lister.quick.folder")}</p>;
  else if (current === null) body = <p className="pane-msg">{tr("lister.quick.none")}</p>;
  else body = <FileViewer key={current} path={current} name={baseName(current)} prefs={prefs} onPrefs={setPrefs} />;

  return (
    <div className="quick" role="region" aria-label={tr("lister.quick")} onMouseDown={(event) => event.stopPropagation()}>
      {body}
    </div>
  );
}
