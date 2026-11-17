import { useCallback, useEffect, useRef, useState } from "react";
import { useAppState, useController, useLanguage, useRuntime, useT } from "../../app/context";
import { formatBytes, formatDate, type TranslationKey } from "../../i18n";
import type { DupesPage, KeepSuggestion, KeepRule, ScanProgress, UsageEntry, UsagePage } from "../../ipc/types";
import { parentPath, baseName } from "../../state/paths";
import { Modal } from "../Modal";
import { useDialogFocus } from "./useDialogFocus";
const terminal = (s: string) => ["done", "failed", "cancelled"].includes(s);
/** Polling also reconciles terminal events emitted before the start command resolves. */
export function useScan(kind: "usage" | "dupes" | "similar") {
  const {
    api
  } = useRuntime();
  const [id, setId] = useState<number | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const live = useRef(true),
    generation = useRef(0),
    held = useRef<number | null>(null);
  const close = useCallback(async () => {
    generation.current++;
    const old = held.current;
    held.current = null;
    if (old !== null) await (kind === "usage" ? api.usageClose(old) : kind === "similar" ? api.similarClose(old) : api.dupesClose(old));
  }, [api, kind]);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
      void close().catch(() => {});
    };
  }, [close]);
  const start = async (submit: () => Promise<number>) => {
    if (!live.current) return;
    setError("");
    setRunning(true);
    try {
      setId(null);
      await close();
      const gen = generation.current;
      const next = await submit();
      if (!live.current || gen !== generation.current) {
        await (kind === "usage" ? api.usageClose(next) : kind === "similar" ? api.similarClose(next) : api.dupesClose(next));
        return;
      }
      held.current = next;
      setId(next);
    } catch (e) {
      if (live.current) {
        setError(String(e));
        setRunning(false);
      }
    }
  };
  const cancel = async () => {
    if (id !== null) try {
      await api.cancelJob(id);
    } catch (e) {
      setError(String(e));
    }
  };
  return {
    id,
    isCurrent: (requestId: number) => live.current && held.current === requestId,
    running,
    setRunning,
    error,
    setError,
    start,
    cancel
  };
}
function Progress({
  progress,
  cancel
}: {
  progress?: ScanProgress;
  cancel: () => void;
}) {
  const tr = useT(),
    language = useLanguage();
  return <div className="cmp-run"><progress aria-label={tr("space.progress")} />
    <span>{tr((`space.stage.${progress?.stage || "size"}` as TranslationKey))} · {tr("space.progressCount", {
        count: progress?.entries ?? 0,
        size: formatBytes(progress?.bytes ?? 0, language)
      })}</span>
    <button className="tb" onClick={cancel}>{tr("space.cancel")}</button></div>;
}
export function Issues({
  issues,
  incomplete,
  count
}: {
  issues: [string, string][];
  incomplete: boolean;
  count?: number;
}) {
  const tr = useT();
  return <>{incomplete && <p role="status">{tr("space.incomplete")}</p>}{(count ?? issues.length) > 0 && <details><summary>{tr("space.issues", {
          count: count ?? issues.length
        })}</summary><ul>{issues.map(([path, reason], i) => <li key={i}>{path}: {tr((`space.reason.${reason}` as TranslationKey))}</li>)}</ul></details>}</>;
}
export function Confirmation({
  count,
  size,
  name,
  similar = false,
  names,
  confirmDefault = false,
  confirm,
  cancel
}: {
  count: number;
  size: number;
  name?: string;
  similar?: boolean;
  names?: string[];
  /** Enter bestätigt (Fokus auf der Bestätigen-Schaltfläche) statt abzubrechen. */
  confirmDefault?: boolean;
  confirm: () => void;
  cancel: () => void;
}) {
  const tr = useT(),
    language = useLanguage();
  return <Modal role="alertdialog" title={tr("dialog.trash.title")} onClose={cancel} footer={<><button className="tb" data-autofocus={confirmDefault ? undefined : true} onClick={cancel}>{tr("action.cancel")}</button><button className="tb danger" data-autofocus={confirmDefault ? true : undefined} onClick={confirm}>{tr("dialog.trash.confirm")}</button></>}><p>{name || names ? tr("dialog.trash.body", {
        name: name ?? names?.[0] ?? "",
        count: names ? count : 1
      }) : tr(similar ? "similar.selectionSummary" : "space.excess", {
        count,
        size: formatBytes(size, language)
      })}</p>{names && <ul>{names.slice(0, 20).map(path => <li key={path}>{path}</li>)}{names.length > 20 && <li>{tr("dialog.trash.more", { count: names.length - 20 })}</li>}</ul>}<p>{tr("space.undoHint")}</p></Modal>;
}
export function UsageDialog({
  root,
  onClose
}: {
  root: string;
  onClose: () => void;
}) {
  const {
      api
    } = useRuntime(),
    controller = useController(),
    active = useAppState(s => s.active),
    tr = useT(),
    language = useLanguage();
  const scan = useScan("usage"),
    anchor = useRef<HTMLDivElement>(null);
  const [page, setPage] = useState<UsagePage | null>(null),
    [oneFileSystem, setOne] = useState(true),
    [trail, setTrail] = useState<string[]>([]),
    [selected, setSelected] = useState<UsageEntry | null>(null),
    [confirm, setConfirm] = useState(false),
    [busy, setBusy] = useState(false);
  const nodePath = trail.at(-1);
  useDialogFocus(anchor, `${scan.running}:${busy}`);
  const start = () => {
    setPage(null);
    setSelected(null);
    setTrail([]);
    return scan.start(() => api.usageStart(root, {
      oneFileSystem
    }));
  };
  useEffect(() => {
    if (scan.id === null) return;
    let dead = false,
      timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const p = await api.usageNode(scan.id!, nodePath);
        const status = (await api.jobs()).find(j => j.id === scan.id);
        if (dead || !scan.isCurrent(scan.id!)) return;
        setPage(p);
        const done = p.ready || !!status && terminal(status.state);
        scan.setRunning(!done);
        if (status?.state === "failed" || status?.state === "cancelled") scan.setError(tr(status.state === "cancelled" ? "space.cancelled" : "space.failed"));
        if (!done) timer = setTimeout(() => void poll(), 150);
      } catch (e) {
        if (!dead) {
          scan.setRunning(false);
          scan.setError(String(e));
        }
      }
    };
    void poll();
    return () => {
      dead = true;
      clearTimeout(timer);
    };
  }, [api, scan.id, nodePath, language]);
  const descend = (entry: UsageEntry) => {
    if (entry.totals.directories > 0) {
      setTrail(t => [...t, entry.path]);
      setSelected(null);
    }
  };
  const back = () => {
    setTrail(t => t.slice(0, -1));
    setSelected(null);
  };
  const remove = async () => {
    if (!selected) return;
    setConfirm(false);
    setBusy(true);
    try {
      const id = await api.trash([selected.path]);
      const job = await waitJob(api, id);
      await start();
      if (job.state !== "done" || job.failed.length) scan.setError(tr("space.trashFailed") + " " + job.failed.map(e => e.message).join("; "));
    } catch (e) {
      scan.setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return <><Modal title={tr("space.title")} className="space-dialog" onClose={onClose} footer={<><button className="tb" disabled={!selected || busy} onClick={() => {
        if (selected) {
          void controller.navigateTo(active, selected.totals.directories ? selected.path : parentPath(selected.path));
          onClose();
        }
      }}>{tr("space.open")}</button><button className="tb danger" disabled={!selected || busy || scan.running} onClick={() => setConfirm(true)}>{tr("action.trash")}</button></>}>
    <div ref={anchor}><p className="space-path">{root}</p><label className="check"><input type="checkbox" checked={oneFileSystem} disabled={scan.running || busy} onChange={e => setOne(e.target.checked)} />{tr("space.oneFileSystem")}</label>
    <button className="tb" data-autofocus disabled={scan.running || busy} onClick={start}>{tr(page ? "space.again" : "space.start")}</button>
    {scan.running && <Progress progress={page?.progress} cancel={() => void scan.cancel()} />}{scan.error && <p role="alert" className="field-error">{scan.error}</p>}
    {page?.ready && <><p title={tr("space.sizes", {
            allocated: formatBytes(page.totals?.allocated ?? 0, language),
            logical: formatBytes(page.totals?.size ?? 0, language)
          })}>{tr("space.summary", {
              size: formatBytes(page.totals?.allocated ?? 0, language),
              count: page.totals?.files ?? 0,
              seconds: page.seconds.toLocaleString(language, {
                maximumFractionDigits: 1
              })
            })}</p>
      <nav aria-label={tr("space.breadcrumb")}><button className="tb" disabled={!trail.length} onClick={back}>{tr("space.back")}</button><button className="tb" onClick={() => {
              setTrail([]);
              setSelected(null);
            }}>{root}</button>{trail.map((p, i) => <button className="tb" key={p} onClick={() => {
              setTrail(t => t.slice(0, i + 1));
              setSelected(null);
            }}>{baseName(p)}</button>)}</nav>
      <div role="listbox" tabIndex={0} aria-label={tr("space.title")} aria-activedescendant={selected ? `usage-${page.children.indexOf(selected)}` : undefined} className="space-bars" onKeyDown={e => {
            const rows = page.children,
              index = selected ? rows.findIndex(r => r.path === selected.path) : -1;
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setSelected(rows[Math.max(0, Math.min(rows.length - 1, index + (e.key === "ArrowDown" ? 1 : -1)))] ?? null);
            }
            if (e.key === "Enter" && selected) {
              e.preventDefault();
              descend(selected);
            }
            if (e.key === "Backspace") {
              e.preventDefault();
              back();
            }
          }}>{page.children.map((entry, i) => {
              const percent = 100 * entry.totals.allocated / Math.max(1, page.totals?.allocated ?? 0);
              return <div id={`usage-${i}`} key={entry.path} role="option" aria-selected={selected?.path === entry.path} className="space-row" onClick={() => setSelected(entry)} onDoubleClick={() => descend(entry)} title={tr("space.sizes", {
                allocated: formatBytes(entry.totals.allocated, language),
                logical: formatBytes(entry.totals.size, language)
              })}><span>{baseName(entry.path)}</span><span>{formatBytes(entry.totals.allocated, language)}</span><span>{percent.toFixed(1)} %</span><span>{tr("space.files", {
                    count: entry.totals.files
                  })}</span><span className="space-bar" style={{
                  width: `${percent}%`
                }} /></div>;
            })}
      {page.other && <div role="option" aria-disabled="true" aria-selected="false" className="space-row">{tr("space.otherLabel")} · {formatBytes(page.other.allocated, language)}</div>}</div><Issues issues={page.issues} count={page.skippedCount} incomplete={page.incomplete} /></>}
    </div></Modal>{confirm && selected && <Confirmation count={1} name={baseName(selected.path)} size={selected.totals.allocated} confirm={() => void remove()} cancel={() => setConfirm(false)} />}</>;
}
export async function waitJob(api: ReturnType<typeof useRuntime>["api"], id: number) {
  for (;;) {
    const job = (await api.jobs()).find(j => j.id === id);
    if (job && terminal(job.state)) {
      return job;
    }
    await new Promise(r => setTimeout(r, 150));
  }
}
export function DupesDialog({
  root,
  onClose
}: {
  root: string;
  onClose: () => void;
}) {
  const {
      api
    } = useRuntime(),
    tr = useT(),
    language = useLanguage(),
    scan = useScan("dupes"),
    anchor = useRef<HTMLDivElement>(null);
  const [roots, setRoots] = useState([root]),
    [minSize, setMin] = useState(1),
    [page, setPage] = useState<DupesPage | null>(null),
    [choices, setChoices] = useState<KeepSuggestion[]>([]),
    [rule, setRule] = useState<KeepRule>("oldest"),
    [confirm, setConfirm] = useState(false),
    [busy, setBusy] = useState(false),
    [skipped, setSkipped] = useState<{
      path: string;
      reason: string;
    }[]>([]);
  useDialogFocus(anchor, `${scan.running}:${busy}`);
  const start = () => {
    setPage(null);
    setChoices([]);
    return scan.start(() => api.dupesStart(roots, {
      minSize
    }));
  };
  useEffect(() => {
    if (scan.id === null) return;
    let dead = false,
      timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const p = await api.dupesPage(scan.id!, 0, 50);
        const status = (await api.jobs()).find(j => j.id === scan.id);
        if (dead || !scan.isCurrent(scan.id!)) return;
        if (p.cancelled || status?.state === "failed" || status?.state === "cancelled") {
          setPage(null);
          setChoices([]);
          scan.setRunning(false);
          scan.setError(tr(status?.state === "failed" ? "space.failed" : "dupes.cancelled"));
          return;
        }
        setPage(p);
        const done = p.ready && (!status || status.state === "done");
        scan.setRunning(!done);
        if (done) {
          const c = await api.dupesSuggest(scan.id!, rule, root);
          if (!dead && scan.isCurrent(scan.id!)) setChoices(c);
        }
        if (!done) timer = setTimeout(() => void poll(), 150);
      } catch (e) {
        if (!dead) {
          setPage(null);
          setChoices([]);
          scan.setError(String(e));
          scan.setRunning(false);
        }
      }
    };
    void poll();
    return () => {
      dead = true;
      clearTimeout(timer);
    };
    // Rule changes are applied explicitly to avoid resetting pages or user choices.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, scan.id, language]);
  const applyRule = async (value: KeepRule) => {
    setRule(value);
    if (scan.id === null || !page?.ready) return;
    setBusy(true);
    try {
      setChoices(await api.dupesSuggest(scan.id, value, root));
    } catch (e) {
      scan.setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const load = async () => {
    if (scan.id === null || !page) return;
    setBusy(true);
    try {
      const next = await api.dupesPage(scan.id, page.groups.length, 50);
      setPage({
        ...next,
        groups: [...page.groups, ...next.groups]
      });
    } catch (e) {
      scan.setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const excessCount = choices.reduce((n, c) => n + c.redundantCount, 0);
  const excessBytes = choices.reduce((n, c) => n + c.redundantBytes, 0);
  const remove = async () => {
    if (scan.id === null) return;
    setConfirm(false);
    setBusy(true);
    try {
      const result = await api.dupesTrash(scan.id, choices.map(({
        groupIndex,
        keepPath
      }) => ({
        groupIndex,
        keepPath
      })));
      setSkipped(result.skipped);
      const job = await waitJob(api, result.jobId);
      setSkipped(s => [...s, ...job.warnings.map(w => ({
        path: w.path,
        reason: w.message
      }))]);
      await start();
      if (job.state !== "done" || job.failed.length) scan.setError(tr("space.trashFailed") + " " + job.failed.map(e => e.message).join("; "));
    } catch (e) {
      scan.setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return <><Modal title={tr("dupes.title")} className="space-dialog" onClose={onClose} footer={<><span>{tr("space.excess", {
          count: excessCount,
          size: formatBytes(excessBytes, language)
        })}</span><button className="tb danger" disabled={!page?.ready || !excessCount || !choices.length || busy || scan.running} onClick={() => setConfirm(true)}>{tr("dupes.trash")}</button></>}><div ref={anchor}>
    <p>{tr("dupes.local")}</p><fieldset disabled={scan.running || busy}><legend>{tr("dupes.roots")}</legend>{roots.map((r, i) => <div className="space-root" key={i}><input aria-label={tr("dupes.root", {
              count: i + 1
            })} value={r} onChange={e => setRoots(rs => rs.map((p, j) => j === i ? e.target.value : p))} /><button className="tb" disabled={roots.length === 1} onClick={() => setRoots(rs => rs.filter((_, j) => i !== j))}>{tr("dupes.removeRoot")}</button></div>)}<button className="tb" onClick={() => setRoots(rs => [...rs, root])}>{tr("dupes.addRoot")}</button><label>{tr("dupes.minSize")}<input type="number" min={0} value={minSize} onChange={e => setMin(Number(e.target.value))} /></label><button className="tb" data-autofocus onClick={start}>{tr("dupes.start")}</button></fieldset>
    {scan.running && <Progress progress={page?.progress} cancel={() => void scan.cancel()} />}{scan.error && <p role="alert" className="field-error">{scan.error}</p>}
    {page?.ready && <><p>{tr("dupes.summary", {
              count: page.total,
              size: formatBytes(page.reclaimableBytes, language)
            })}</p><label>{tr("dupes.keep")}<select disabled={busy} value={rule} onChange={e => void applyRule((e.target.value as KeepRule))}>{(["oldest", "newest", "shortestPath", "inFolder"] as const).map(r => <option key={r} value={r}>{tr(`dupes.rule.${r}`)}</option>)}</select></label>
      <ul className="dupe-groups">{page.groups.map(g => <li key={g.groupIndex}><fieldset disabled={busy}><legend>{tr("dupes.group", {
                    size: formatBytes(g.size, language),
                    count: g.entries.length,
                    reclaimable: formatBytes(g.reclaimableBytes, language)
                  })}</legend>{g.entries.map(e => <label className="dupe-entry" key={e.path}><input type="radio" name={`keep-${g.groupIndex}`} checked={choices.find(c => c.groupIndex === g.groupIndex)?.keepPath === e.path} onChange={() => setChoices(cs => [...cs.filter(c => c.groupIndex !== g.groupIndex), {
                    groupIndex: g.groupIndex,
                    keepPath: e.path,
                    redundantCount: g.entries.filter(other => !other.protected && other.path !== e.path).length,
                    redundantBytes: g.entries.filter(other => !other.protected && other.path !== e.path).length * g.size
                  }])} /><span>{e.path}<small>{formatDate(e.modified, language)} {e.protected ? tr("dupes.protected") : ""}</small></span></label>)}</fieldset></li>)}</ul>
      {page.groups.length < page.total && <button className="tb" disabled={busy} onClick={() => void load()}>{tr("dupes.more")}</button>}<Issues issues={page.issues} incomplete={page.incomplete} /></>}
    {skipped.length > 0 && <details open><summary>{tr("dupes.skipped", {
              count: skipped.length
            })}</summary><ul>{skipped.map((s, i) => <li key={i}>{s.path}: {tr((`space.reason.${s.reason}` as TranslationKey))}</li>)}</ul></details>}
    </div></Modal>{confirm && <Confirmation count={excessCount} size={excessBytes} confirm={() => void remove()} cancel={() => setConfirm(false)} />}</>;
}
