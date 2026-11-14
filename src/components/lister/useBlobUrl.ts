import { useCallback, useEffect, useRef, useState } from "react";
import { useController } from "../../app/context";
import type { FileInfo } from "../../ipc/types";
import { blobHolder, type BlobLease } from "./blobHolder";
import { loadBlob, type Cancel } from "./lib";

export type BlobState = { phase: "idle" } | { phase: "loading" } | { phase: "ready"; url: string } | { phase: "error"; message: string };

/**
 * Liest die Datei blockweise zu einem Blob und macht ihn über den `blobHolder` zum aktiven
 * (höchstens einer zugleich). Beim Entfernen der Ansicht wird die URL freigegeben.
 * Mit `auto` startet das Laden von selbst, sonst erst über `load()`.
 */
export function useBlobUrl(path: string, info: FileInfo, auto: boolean): { state: BlobState; load(): void } {
  const { api, describeError } = useController();
  const [state, setState] = useState<BlobState>({ phase: "idle" });
  const lease = useRef<BlobLease | null>(null);
  const cancel = useRef<Cancel>({ cancelled: false });
  const started = useRef(false);

  const load = useCallback(() => {
    if (started.current) return;
    started.current = true;
    const mine = cancel.current;
    setState({ phase: "loading" });
    void (async () => {
      try {
        const blob = await loadBlob(api, path, info.size, info.mimeGuess, mine);
        if (!blob || mine.cancelled) return;
        lease.current = blobHolder.acquire(blob);
        setState({ phase: "ready", url: lease.current.url });
      } catch (error) {
        if (!mine.cancelled) setState({ phase: "error", message: describeError(error, { path }) });
      }
    })();
  }, [api, path, info.size, info.mimeGuess, describeError]);

  useEffect(() => {
    const mine = { cancelled: false };
    cancel.current = mine;
    started.current = false;
    if (auto) load();
    return () => {
      mine.cancelled = true;
      lease.current?.release();
      lease.current = null;
    };
  }, [auto, load]);

  return { state, load };
}
