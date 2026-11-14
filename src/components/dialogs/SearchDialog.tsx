import { useEffect, useRef, useState } from "react";
import { useAppState, useController, useT } from "../../app/context";
import { activeTabOf, type WinIdx } from "../../app/state";
import { baseName, displayPath, parentPath } from "../../state/paths";
import type { SearchHit, SearchOptions, SearchSummary } from "../../ipc/types";
import { Modal } from "../Modal";

const PAGE = 100;

/** Dateien im aktuellen Ordner nach Name und Inhalt durchsuchen; ein Treffer öffnet seinen Ordner. */
export function SearchDialog({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const connections = useAppState((state) => state.remoteConnections);
  const root = useAppState((state) => activeTabOf(state, w).path);

  const [name, setName] = useState("");
  const [text, setText] = useState("");
  const [textRegex, setTextRegex] = useState(false);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [recursive, setRecursive] = useState(true);
  const [inArchives, setInArchives] = useState(false);

  const [jobId, setJobId] = useState<number | null>(null);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [summary, setSummary] = useState<SearchSummary | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const jobRef = useRef<number | null>(null);
  jobRef.current = jobId;
  // Beim Schließen einen laufenden Suchauftrag freigeben.
  useEffect(() => () => { if (jobRef.current !== null) void controller.api.searchClose(jobRef.current).catch(() => {}); }, [controller]);

  // Die Suche ist ein Hintergrundauftrag; die Treffer stehen erst nach „done" bereit.
  const jobState = useAppState((state) => (jobId !== null ? state.jobs.jobs.find((job) => job.id === jobId)?.state : undefined));
  const pagedFor = useRef<number | null>(null);
  useEffect(() => {
    if (jobId === null || pagedFor.current === jobId) return;
    if (jobState === "done") {
      pagedFor.current = jobId;
      void page(jobId, 0)
        .catch((err) => setError(err instanceof Error ? err.message : tr("error.unknown")))
        .finally(() => setRunning(false));
    } else if (jobState === "failed" || jobState === "cancelled") {
      pagedFor.current = jobId;
      setRunning(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jobId, jobState]);

  const closeJob = async () => {
    if (jobRef.current !== null) {
      const id = jobRef.current;
      setJobId(null);
      jobRef.current = null;
      await controller.api.searchClose(id).catch(() => {});
    }
  };

  const page = async (id: number, offset: number) => {
    const result = await controller.api.searchPage(id, offset, PAGE);
    setHits((previous) => (offset === 0 ? result.hits : [...previous, ...result.hits]));
    setSummary(result.summary);
  };

  const run = async () => {
    if (!name.trim() && !text.trim()) return;
    setError(null);
    setRunning(true);
    setHits([]);
    setSummary(null);
    await closeJob();
    const options: SearchOptions = {
      name: name.trim() || null,
      text: text.trim() || null,
      textRegex,
      nameCaseSensitive: caseSensitive,
      textCaseSensitive: caseSensitive,
      recursive,
      inArchives,
    };
    try {
      const id = await controller.api.searchStart(root, options);
      pagedFor.current = null;
      setJobId(id);
      jobRef.current = id;
      // Treffer werden abgerufen, sobald der Auftrag „done" meldet (siehe Effekt oben).
    } catch (err) {
      setError(err instanceof Error ? err.message : tr("error.unknown"));
      setRunning(false);
    }
  };

  const openHit = (hit: SearchHit) => {
    void controller.api.searchClose(jobRef.current ?? -1).catch(() => {});
    jobRef.current = null;
    controller.closeDialog();
    void controller.navigateTo(w, parentPath(hit.path), baseName(hit.path));
  };

  return (
    <Modal title={tr("search.title")} onClose={() => controller.closeDialog()} wide>
      <form
        className="search-form"
        onSubmit={(event) => { event.preventDefault(); void run(); }}
      >
        <label className="search-row">
          <span>{tr("search.name")}</span>
          <input type="text" value={name} data-autofocus onChange={(e) => setName(e.target.value)} placeholder={tr("search.namePlaceholder")} />
        </label>
        <label className="search-row">
          <span>{tr("search.text")}</span>
          <input type="text" value={text} onChange={(e) => setText(e.target.value)} placeholder={tr("search.textPlaceholder")} />
        </label>
        <div className="search-opts">
          <label><input type="checkbox" checked={textRegex} onChange={(e) => setTextRegex(e.target.checked)} /> {tr("search.regex")}</label>
          <label><input type="checkbox" checked={caseSensitive} onChange={(e) => setCaseSensitive(e.target.checked)} /> {tr("search.case")}</label>
          <label><input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} /> {tr("search.recursive")}</label>
          <label><input type="checkbox" checked={inArchives} onChange={(e) => setInArchives(e.target.checked)} /> {tr("search.archives")}</label>
          <button type="submit" className="tb primary" disabled={running || (!name.trim() && !text.trim())}>{tr("search.run")}</button>
        </div>
      </form>

      {error ? <p role="alert" className="search-error">{error}</p> : null}

      <div className="search-results" role="listbox" aria-label={tr("search.results")}>
        {hits.map((hit, index) => (
          <button type="button" role="option" aria-selected="false" key={`${hit.path}:${hit.line ?? ""}:${index}`} className="search-hit" onClick={() => openHit(hit)}>
            <span className="search-hit-name">{baseName(hit.path)}</span>
            <span className="search-hit-dir">{displayPath(parentPath(hit.path), connections)}</span>
            {hit.line !== null ? <span className="search-hit-line">{tr("search.line", { line: hit.line })}</span> : null}
            {hit.excerpt ? <span className="search-hit-excerpt">{hit.excerpt}</span> : null}
          </button>
        ))}
      </div>

      {summary ? (
        <p className="search-summary">
          {tr("search.summary", { total: summary.total, files: summary.filesSearched })}
          {summary.incomplete ? ` · ${tr("search.incomplete")}` : ""}
          {jobId !== null && hits.length < summary.total ? (
            <button type="button" className="tb small" onClick={() => void page(jobId, hits.length)}>{tr("search.more")}</button>
          ) : null}
        </p>
      ) : running ? <p className="search-summary">{tr("search.running")}</p> : null}
    </Modal>
  );
}
