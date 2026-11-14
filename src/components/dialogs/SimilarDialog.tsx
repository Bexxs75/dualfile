import { useEffect, useRef, useState } from "react";
import { useLanguage, useRuntime, useT } from "../../app/context";
import { formatBytes, formatDate, type TranslationKey } from "../../i18n";
import type { SimilarPage, SimilarRule, SimilarEntry, SimilarGroup } from "../../ipc/types";
import { baseName } from "../../state/paths";
import { Modal } from "../Modal";
import { Confirmation, Issues, useScan, waitJob } from "./SpaceDialogs";
import { useDialogFocus } from "./useDialogFocus";

// Shared by visible cards across dialogs; requests queued offscreen are discarded.
let activeThumbnails = 0;
const thumbnailQueue: (() => void)[] = [];
function drain() {
  while (activeThumbnails < 2 && thumbnailQueue.length) thumbnailQueue.shift()!();
}
function Preview({
  id,
  group,
  index,
  entry
}: {
  id: number;
  group: number;
  index: number;
  entry: SimilarEntry;
}) {
  const {
      api
    } = useRuntime(),
    tr = useT();
  const ref = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState(""),
    [error, setError] = useState(false);
  useEffect(() => {
    let live = true,
      visible = false,
      queued = false,
      started = false;
    const run = () => {
      queued = false;
      if (!live || !visible || started) return;
      started = true;
      activeThumbnails++;
      void api.similarThumbnail(id, group, index).then(t => {
        if (live) setSrc("data:image/png;base64," + btoa(t.bytes.map(b => String.fromCharCode(b)).join("")));
      }).catch(() => {
        if (live) setError(true);
      }).finally(() => {
        activeThumbnails--;
        drain();
      });
    };
    const observe = (v: boolean) => {
      visible = v;
      if (v && !queued && !started) {
        queued = true;
        thumbnailQueue.push(run);
        drain();
      }
    };
    const observer = typeof IntersectionObserver !== "undefined" ? new IntersectionObserver(es => observe(es.some(e => e.isIntersecting))) : null;
    if (observer && ref.current) observer.observe(ref.current);else observe(true);
    return () => {
      live = false;
      observer?.disconnect();
      const i = thumbnailQueue.indexOf(run);
      if (i >= 0) thumbnailQueue.splice(i, 1);
    };
  }, [api, id, group, index]);
  return <div ref={ref} className="similar-preview">{src ? <img src={src} alt={baseName(entry.path)} /> : <span role="img" aria-label={tr(error ? "similar.previewError" : "similar.previewLoading")}>{error ? "⚠" : "…"}</span>}</div>;
}
export function SimilarDialog({
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
    scan = useScan("similar"),
    anchor = useRef<HTMLDivElement>(null);
  const [roots, setRoots] = useState([root]),
    [minSize, setMin] = useState(0),
    [distance, setDistance] = useState(6);
  const [page, setPage] = useState<SimilarPage | null>(null),
    [selected, setSelected] = useState<Set<string>>(new Set());
  const [rule, setRule] = useState<SimilarRule>("largestResolution"),
    [busy, setBusy] = useState(false),
    [confirm, setConfirm] = useState(false),
    [paused, setPaused] = useState(false);
  const [skipped, setSkipped] = useState<{
    path: string;
    reason: string;
  }[]>([]);
  useDialogFocus(anchor, `${scan.running}:${busy}`);
  const start = () => {
    setPage(null);
    setSelected(new Set());
    setPaused(false);
    return scan.start(() => api.similarStart(roots, {
      minSize,
      maxDistance: distance
    }));
  };
  useEffect(() => {
    if (scan.id === null) return;
    const id = scan.id;
    let dead = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const p = await api.similarPage(id, 0, 50);
        const status = (await api.jobs()).find(j => j.id === id);
        if (dead || !scan.isCurrent(id)) return;
        if (p.cancelled || status?.state === "failed" || status?.state === "cancelled") {
          setPage(null);
          setSelected(new Set());
          scan.setRunning(false);
          scan.setError(tr(status?.state === "failed" ? "space.failed" : "dupes.cancelled"));
          return;
        }
        setPage(p);
        setPaused(status?.state === "paused");
        const done = p.ready && (!status || status.state === "done");
        scan.setRunning(!done);
        if (!done) timer = setTimeout(() => void poll(), 150);
      } catch (e) {
        if (!dead) {
          setPage(null);
          setSelected(new Set());
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
  }, [api, scan.id, language]);
  const action = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      scan.setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const toggle = (g: SimilarGroup, e: SimilarEntry) => setSelected(old => {
    const next = new Set(old);
    if (next.has(e.path)) next.delete(e.path);else if (!e.protected && g.entries.filter(e => !next.has(e.path)).length > 1) next.add(e.path);
    return next;
  });
  const markSuggestion = (g: SimilarGroup) => action(async () => {
    const id = scan.id!;
    const choices = await api.similarSuggest(id, rule);
    if (!scan.isCurrent(id)) return;
    const keep = choices.find(c => c.groupIndex === g.groupIndex)?.keepPath;
    if (!keep || !g.entries.some(e => e.path === keep)) return;
    setSelected(old => {
      const next = new Set(old);
      for (const e of g.entries) {
        next.delete(e.path);
        if (e.path !== keep && !e.protected) next.add(e.path);
      }
      return next;
    });
  });
  const chosen = page?.groups.flatMap(g => g.entries.filter(e => selected.has(e.path))) ?? [];
  const size = chosen.reduce((n, e) => n + e.size, 0);
  const remove = () => action(async () => {
    setConfirm(false);
    const id = scan.id!;
    const r = await api.similarTrash(id, page!.groups.map(g => ({
      groupIndex: g.groupIndex,
      trashPaths: g.entries.filter(e => selected.has(e.path)).map(e => e.path)
    })).filter(c => c.trashPaths.length));
    if (!scan.isCurrent(id)) return;
    setSkipped(r.skipped);
    setSelected(new Set());
    const job = await waitJob(api, r.jobId);
    if (!scan.isCurrent(id)) return;
    setSkipped(s => [...s, ...job.warnings.map(w => ({
      path: w.path,
      reason: w.message
    }))]);
    await start();
    if (job.state !== "done" || job.failed.length) scan.setError(tr("space.trashFailed"));
  });
  return <><Modal title={tr("similar.title")} className="space-dialog" onClose={onClose} footer={<><span>{tr("similar.selectionSummary", {
          count: chosen.length,
          size: formatBytes(size, language)
        })}</span><button className="tb danger" disabled={!page?.ready || !chosen.length || busy || scan.running} onClick={() => setConfirm(true)}>{tr("similar.trash")}</button></>}><div ref={anchor}>
    <p>{tr("similar.local")}</p><p>{tr("similar.warning")}</p>
    <fieldset disabled={busy || scan.running}><legend>{tr("dupes.roots")}</legend>{roots.map((r, i) => <div className="space-root" key={i}><input aria-label={tr("dupes.root", {
              count: i + 1
            })} value={r} onChange={e => setRoots(rs => rs.map((v, j) => i === j ? e.target.value : v))} /><button className="tb" disabled={roots.length === 1} onClick={() => setRoots(rs => rs.filter((_, j) => j !== i))}>{tr("dupes.removeRoot")}</button></div>)}<button className="tb" onClick={() => setRoots(rs => [...rs, root])}>{tr("dupes.addRoot")}</button>
    <label className="fld">{tr("dupes.minSize")}<input type="number" min={0} value={minSize} onChange={e => setMin(Number(e.target.value))} /></label>
    <label className="fld">{tr("similar.sensitivity")}<select value={distance} onChange={e => setDistance(Number(e.target.value))}>{([2, 6, 10] as const).map(n => <option key={n} value={n}>{tr(`similar.level.${n}`)}</option>)}</select></label>
    <button className="tb" data-autofocus onClick={start}>{tr("dupes.start")}</button></fieldset>
    {scan.running && <div className="cmp-run"><progress aria-label={tr("space.progress")} max={page?.progress.total ?? undefined} value={page?.progress.total ? page.progress.entries : undefined} /><span>{tr((`space.stage.${page?.progress.stage || "scan"}` as TranslationKey))} · {page?.progress.entries ?? 0} / {page?.progress.total ?? "…"}</span><button className="tb" onClick={() => void action(async () => {
            if (scan.id !== null) await (paused ? api.jobResume(scan.id) : api.jobPause(scan.id));
          })}>{tr(paused ? "similar.resume" : "similar.pause")}</button><button className="tb" onClick={() => void scan.cancel()}>{tr("space.cancel")}</button></div>}
    {scan.error && <p role="alert" className="field-error">{scan.error}</p>}
    {page?.ready && <>{page.total === 0 && <p role="status">{tr("similar.empty")}</p>}<p>{tr("similar.groups", {
              count: page.total
            })}, {tr("similar.images", {
              count: page.images
            })} · {formatBytes(page.reclaimableBytes, language)}</p>
      <p>{tr("similar.keepOne")}</p><label className="fld">{tr("dupes.keep")}<select disabled={busy} value={rule} onChange={e => setRule((e.target.value as SimilarRule))}>{(["largestResolution", "largestFile", "oldest", "newest", "shortestPath"] as const).map(r => <option key={r} value={r}>{tr(`similar.rule.${r}`)}</option>)}</select></label>
      <ul className="dupe-groups">{page.groups.map(g => <li key={g.groupIndex}><fieldset disabled={busy}><legend>{tr("similar.images", {
                    count: g.entries.length
                  })}</legend>
        <button className="tb" onClick={() => void markSuggestion(g)}>{tr("similar.apply")}</button><button className="tb" onClick={() => setSelected(old => new Set([...old].filter(p => !g.entries.some(e => e.path === p))))}>{tr("similar.clear")}</button>
        <div className="similar-cards">{g.entries.map((e, i) => {
                    const locked = e.protected || !selected.has(e.path) && g.entries.filter(e => !selected.has(e.path)).length <= 1;
                    return <article key={e.path} className="similar-card" tabIndex={0} aria-label={baseName(e.path)} onKeyDown={event => {
                      if (event.target !== event.currentTarget) return;
                      if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) {
                        event.preventDefault();
                        const cards = event.currentTarget.parentElement!.querySelectorAll<HTMLElement>("article");
                        cards[Math.max(0, Math.min(cards.length - 1, i + (["ArrowLeft", "ArrowUp"].includes(event.key) ? -1 : 1)))]?.focus();
                      }
                      if (event.key === " " && !busy) {
                        event.preventDefault();
                        toggle(g, e);
                      }
                    }}><Preview id={scan.id!} group={g.groupIndex} index={i} entry={e} /><strong>{baseName(e.path)}</strong><p className="space-path">{e.path}</p><small>{e.width} × {e.height} · {formatBytes(e.size, language)} · {formatDate(e.modified, language)}</small><p>{e.distanceToReference === 0 ? tr("similar.sameHash") : tr("similar.hashDistance", {
                          count: e.distanceToReference
                        })}</p><label><input type="checkbox" checked={selected.has(e.path)} disabled={locked || busy} onChange={() => toggle(g, e)} />{tr("action.trash")}</label>{e.protected && <small>{tr("dupes.protected")}</small>}</article>;
                  })}</div>
      </fieldset></li>)}</ul>
      {page.groups.length < page.total && <button className="tb" disabled={busy} onClick={() => void action(async () => {
            const id = scan.id!;
            const p = await api.similarPage(id, page.groups.length, 50);
            if (scan.isCurrent(id)) setPage({
              ...p,
              groups: [...page.groups, ...p.groups]
            });
          })}>{tr("dupes.more")}</button>}
      <Issues issues={page.issues} incomplete={page.incomplete || page.cancelled} /></>}
    {skipped.length > 0 && <details open><summary>{tr("dupes.skipped", {
              count: skipped.length
            })}</summary><ul>{skipped.map((s, i) => <li key={i}>{s.path}: {tr((`space.reason.${s.reason}` as TranslationKey))}</li>)}</ul></details>}
  </div></Modal>{confirm && <Confirmation similar count={chosen.length} size={size} confirm={() => void remove()} cancel={() => setConfirm(false)} />}</>;
}
