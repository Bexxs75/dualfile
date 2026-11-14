import { useCallback, useEffect, useRef, useState } from "react";
import { useController, useT } from "../../app/context";
import { CoreError } from "../../ipc/api";
import type { SearchDirection } from "../../ipc/types";

export interface FileSearch {
  searching: boolean;
  message: string | null;
  /** Nach einem `continue`-Ergebnis: von hier weitersuchen (gleiche Richtung). */
  pending: { next: number; direction: SearchDirection } | null;
  setMessage(text: string | null): void;
  /** Führt eine Suche aus; liefert den Offset des Treffers oder null (Meldung steht dann in `message`). */
  run(needle: Uint8Array, start: number, direction: SearchDirection, ignoreCase: boolean): Promise<number | null>;
  /** Bricht eine laufende Suche ab (Esc). Liefert true, wenn eine lief. */
  cancel(): boolean;
  clearPending(): void;
}

/** Suche in einer Datei über `findInFile`: eine Suche zugleich, abbrechbar, mit „weitersuchen“. */
export function useFileSearch(path: string): FileSearch {
  const { api, describeError } = useController();
  const tr = useT();
  const [searching, setSearching] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [pending, setPending] = useState<FileSearch["pending"]>(null);
  const running = useRef(false);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (running.current) void api.cancelSearch().catch(() => undefined);
    };
  }, [api]);

  const run = useCallback<FileSearch["run"]>(
    async (needle, start, direction, ignoreCase) => {
      if (running.current) return null;
      running.current = true;
      setSearching(true);
      setMessage(null);
      setPending(null);
      try {
        const result = await api.findInFile(path, needle, start, direction, ignoreCase);
        if (!alive.current) return null;
        if (result.kind === "found") return result.offset;
        if (result.kind === "continue") {
          setPending({ next: result.next, direction });
          setMessage(tr("lister.searchContinue", { offset: result.next }));
        } else setMessage(tr("lister.notFound"));
        return null;
      } catch (error) {
        if (alive.current) {
          setMessage(error instanceof CoreError && error.kind === "cancelled" ? tr("lister.searchCancelled") : describeError(error, { path }));
        }
        return null;
      } finally {
        running.current = false;
        if (alive.current) setSearching(false);
      }
    },
    [api, path, describeError, tr],
  );

  const cancel = useCallback((): boolean => {
    if (!running.current) return false;
    void api.cancelSearch().catch(() => undefined);
    return true;
  }, [api]);

  return { searching, message, pending, setMessage, run, cancel, clearPending: () => setPending(null) };
}
