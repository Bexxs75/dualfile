import { formatJobPresentation } from "../state/jobPresentation";
import { useAppState, useController, useLanguage, useT } from "../app/context";
import { formatBytes } from "../i18n";
import { failureReason, isFinished, jobDisplayName, summarizeJob } from "../state/operations";

/** Laufende und fertige Aufträge mit Fortschritt, Abbrechen und der Fehlerliste („3 von 120 nicht kopiert“). */
export function JobsPanel() {
  const controller = useController();
  const tr = useT();
  const language = useLanguage();
  const jobs = useAppState((state) => state.jobs.jobs);
  const expanded = useAppState(state => state.jobsExpanded);
  const active = jobs.filter(job => !isFinished(job.state));
  const allPaused = active.length > 0 && active.every(job => job.state === "paused");
  const conflicts = useAppState(s => s.conflicts);
  const itemErrors = useAppState(s => s.itemErrors);
  const kinds = useAppState((state) => state.jobKinds);
  // Papierkorb-Aufträge sind sofort fertig und stehen als Meldung da, nicht als Auftrag.
  // Vergleiche zeigt ihr Dialog mit eigenem Fortschritt.
  const shown = jobs.filter((job) => kinds[job.id] !== "trash" && kinds[job.id] !== "compare");
  if (shown.length === 0) return null;
  return (
    <section className="jobs" aria-label={tr("jobs.title")}>
      <div className="job-head jobs-header">
        <button className="tb" type="button" aria-expanded={expanded} aria-controls="jobs-list" title={tr("jobs.expand")} onClick={() => controller.setJobsExpanded(true)}>{tr("jobs.title")} · {tr("jobs.active", { count: active.length })} {expanded ? "▾" : "▸"}</button>
        {active.length > 0 ? <button className="tb small" type="button" title={tr(allPaused ? "jobs.resumeAll" : "jobs.pauseAll")} onClick={() => void controller.pauseAllJobs(!allPaused)}>{tr(allPaused ? "jobs.resumeAll" : "jobs.pauseAll")}</button> : null}
        {expanded ? <button type="button" className="tb small" onClick={() => controller.setJobsExpanded(false)}>{tr("jobs.collapse")}</button> : null}
      </div>
      <div id="jobs-list" hidden={!expanded}>
      {shown.map((job) => {
        const info = summarizeJob(job);
        const finished = isFinished(job.state);
        const kind = kinds[job.id];
        const presentation = formatJobPresentation({ job, kind, waiting: [...conflicts, ...itemErrors].some(q => q.jobId === job.id) }, tr);
        const name = kind === "undo" ? tr("action.undo") : jobDisplayName(job, tr);
        const failedText =
          kind === "undo" || info.failedCount === 0
            ? null
            : kind === "delete"
              ? tr("jobs.summary.delete", { deleted: job.filesDone, failed: info.failedCount })
              : tr(
                  kind === "rename"
                    ? "jobs.summary.rename"
                    : kind === "move"
                      ? "jobs.summary.move"
                      : kind === "extract"
                        ? "jobs.summary.extract"
                        : kind === "pack"
                          ? "jobs.summary.pack"
                          : kind === "sync"
                            ? "jobs.summary.sync"
                            : "jobs.summary.copy",
                  {
                  failed: info.failedCount,
                  total: info.totalCount,
                });
        return (
          <div className="job" key={job.id} role="group" aria-label={name} data-state={job.state}>
            <div className="job-head">
              <span className="job-name">{name}</span>
              <span className="job-state" aria-live="polite">{presentation.state}</span>
              {!finished ? <button type="button" className="tb small" title={tr(job.state === "paused" ? "jobs.resume" : "jobs.pause")} onClick={() => void controller.pauseJob(job.id, job.state !== "paused")}>{tr(job.state === "paused" ? "jobs.resume" : "jobs.pause")}</button> : null}
              {finished ? (
                <button type="button" className="tb small" title={tr("jobs.dismiss")} onClick={() => controller.closeJob(job.id)}>
                  {tr("jobs.dismiss")}
                </button>
              ) : (
                <button type="button" className="tb small" title={tr("jobs.cancel")} onClick={() => void controller.cancelJob(job.id)}>
                  {tr("jobs.cancel")}
                </button>
              )}
            </div>
            <progress role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(info.progress * 100)} max={1} value={info.progress} aria-label={`${name}: ${Math.round(info.progress * 100)} %`} />
            <div className="job-meta">
              <span>{presentation.counter}</span>
              {presentation.eta !== null ? <span>{presentation.eta}</span> : null}
              {job.queuePosition !== null ? <span>{tr("jobs.queue", { position: job.queuePosition })}</span> : null}
              {job.bytesTotal > 0 ? (
                <span>{tr("jobs.progress", { done: formatBytes(job.bytesDone, language), total: formatBytes(job.bytesTotal, language) })}</span>
              ) : null}
              {job.state === "running" && job.bytesPerSecond > 0 ? <span>{tr("jobs.speed", { speed: formatBytes(job.bytesPerSecond, language) })}</span> : null}
            </div>
            {failedText ? (
              <details className="job-errors" open={finished}>
                <summary>{failedText}</summary>
                <ul>
                  {info.failures.map((failure, index) => (
                    <li key={`${failure.path}-${index}`}>
                      <code>{failure.path}</code> – {tr("jobs.reason", { reason: failureReason(job.failed[index] ?? failure, tr) })}
                    </li>
                  ))}
                </ul>
              </details>
            ) : null}
          </div>
        );
      })}
      </div>
    </section>
  );
}
