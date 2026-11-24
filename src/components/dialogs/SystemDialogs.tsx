import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useAppState, useLanguage, useRuntime, useT } from "../../app/context";
import { isFinished } from "../../state/operations";
import { useDialogFocus } from "./useDialogFocus";
import { WhatsNewDialog } from "./WhatsNewDialog";
import { currentVersion } from "../../release-notes";
import { Modal } from "../Modal";
import type { AutostartMode } from "../../ipc/system";
import { Highlight } from "../../settings/Highlight";
import config from "../../../src-tauri/tauri.conf.json";

const AUTOSTART_MODES: readonly AutostartMode[] = ["off", "minimized", "shown"];

export function useSystem() {
  const { system } = useRuntime();
  return { system, state: useSyncExternalStore(system.subscribe, system.getSnapshot) };
}

/** Local action failures stay with the control that can retry them. */
function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const run = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true); setError("");
    try { await action(); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  return { busy, error, run };
}

export function SystemSettings({ field, query = "", onReset }: { field?: "autostart" | "startMenu"; query?: string; onReset?: () => void }) {
  const { system, state } = useSystem();
  const tr = useT();
  const { busy, error, run } = useAction();
  const autostart = useAction();
  // The entries can change outside Dualfile (deleted, switched off in the desktop's settings).
  useEffect(() => {
    if (field === "startMenu") return;
    const refresh = () => void system.refreshStatus();
    refresh();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [system, field]);
  const startMenuError = error || state.status?.startMenuError;
  return <>
    <section className="system-group" aria-label={tr(field === "startMenu" ? "system.startMenu" : "system.settingsTitle")}>
      {field !== "autostart" && state.status && state.status.packaging !== "flatpak" && <div className="setting-row">
        <span className="setting-label"><Highlight text={tr("system.startMenu")} query={query} /> <small className="rare">{tr("settings.rare")}</small></span>
        <div className="setting-control">
          <p className="muted"><b className={state.status?.startMenuExists ? "system-present" : undefined}>{tr(!state.status ? "system.loading" : state.status.startMenuExists ? "system.present" : "system.absent")}</b></p>
          <button id="start-menu-action" className="tb" disabled={busy || !state.status} onClick={() => void run(() => system.setStartMenu(!state.status?.startMenuExists))}><Highlight text={tr(state.status?.startMenuExists ? "system.remove" : "system.add")} query={query} /></button>
          {startMenuError && <p role="alert" className="field-error">{startMenuError}</p>}
        </div>
      </div>}
      {field !== "startMenu" && <div className="setting-row">
        <label htmlFor="autostart-mode"><Highlight text={tr("system.autostart")} query={query} /></label>
        <div className="setting-control">
          <select id="autostart-mode" value={state.status?.autostart ?? "off"} disabled={autostart.busy || !state.status} aria-describedby="autostart-help"
            onChange={e => { const mode = e.target.value as AutostartMode; void autostart.run(() => system.setAutostart(mode)); }}>
            {AUTOSTART_MODES.map(mode => <option key={mode} value={mode}>{tr(`system.autostart.${mode}`)}</option>)}
          </select>
          <p id="autostart-help" className="field-help"><Highlight text={tr(state.status?.packaging === "flatpak" ? "system.autostartPortalHelp" : "system.autostartHelp")} query={query} /></p>
          {(autostart.error || state.status?.autostartError) && <p role="alert" className="field-error">{tr("system.autostartFailed", { message: autostart.error || state.status?.autostartError || "" })}</p>}
        </div>
        {onReset && <div className="setting-reset"><span className="modified-dot" title={tr("settings.changed")}><span aria-hidden="true">●</span><span className="sr-only"> {tr("settings.changed")}</span></span><button className="link" type="button" aria-label={`${tr("settings.reset")}: ${tr("system.autostart")}`} onClick={onReset}>{tr("settings.reset")}</button></div>}
      </div>}
    </section>
  </>;
}

const REPO_URL = "https://github.com/Bexxs75/dualfile";

export function AboutSettings({ query = "" }: { query?: string }) {
  const language = useLanguage();
  const { api } = useRuntime();
  const tr = useT();
  const [notes, setNotes] = useState(false);
  const { error, run } = useAction();
  return <section className="system-group" aria-labelledby="about-heading">
    <h3 id="about-heading"><Highlight text={tr("settings.about.heading")} query={query} /></h3>
    <p>Dualfile <span className="system-version">{config.version}</span> · <Highlight text={tr("settings.about.license")} query={query} /></p>
    <div><button className="tb primary" onClick={() => void run(() => api.openUrl(language === "de" ? "https://wiki.dualfileapp.com/de/" : "https://wiki.dualfileapp.com/"))}><Highlight text={tr("settings.wiki")} query={query} /></button></div>
    <div><button className="tb" onClick={() => void run(() => api.openUrl(REPO_URL))}><Highlight text={tr("settings.about.github")} query={query} /></button></div>
    <div><button className="tb" onClick={() => setNotes(true)}><Highlight text={tr("whatsNew.title")} query={query} /></button></div>
    {notes && <WhatsNewDialog versions={[currentVersion]} onClose={() => setNotes(false)} />}
    {error && <p role="alert" className="field-error">{error}</p>}
  </section>;
}

function formatBytes(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function TroubleshootSettings({ query = "" }: { query?: string }) {
  const { api, controller } = useRuntime();
  const tr = useT();
  const { error, run } = useAction();
  const [info, setInfo] = useState<{ path: string; bytes: number } | null>(null);
  useEffect(() => {
    let current = true;
    api.logInfo().then(value => { if (current) setInfo(value); }, () => {});
    return () => { current = false; };
  }, [api]);
  return <section className="system-group" aria-labelledby="troubleshoot-heading">
    <h3 id="troubleshoot-heading"><Highlight text={tr("log.heading")} query={query} /></h3>
    <p><Highlight text={tr("log.intro")} query={query} /></p>
    {info && <p><span className="muted"><Highlight text={tr("log.location")} query={query} /></span> <code>{info.path}</code> <span className="muted">{formatBytes(info.bytes)}</span></p>}
    <div className="system-actions">
      <button className="tb primary" onClick={() => controller.openDialog({ kind: "log", fromSettings: true })}><Highlight text={tr("log.view")} query={query} /></button>
      <button className="tb" onClick={() => void run(() => api.logOpenFolder())}><Highlight text={tr("log.openFolder")} query={query} /></button>
      <button className="tb" onClick={() => void run(async () => api.openUrl(await api.reportIssueUrl()))}><Highlight text={tr("log.report")} query={query} /></button>
    </div>
    <p className="muted"><Highlight text={tr("log.reportHelp")} query={query} /></p>
    {error && <p role="alert" className="field-error">{error}</p>}
  </section>;
}

export function UpdateSettings({ query = "" }: { query?: string }) {
  const { system, state } = useSystem();
  const settings = useAppState(s => s.settings);
  const language = useLanguage();
  const tr = useT();
  if (!state.status) return null;
  if (state.status.packaging === "flatpak") return <p>{tr("updates.flatpak")}</p>;
  return <>
    <section className="system-group" aria-labelledby="updates-heading">
      <h3 id="updates-heading"><Highlight text={tr("updates.title")} query={query} /></h3>
      <p><Highlight text={tr("updates.version")} query={query} /> <span className="system-version">{config.version}</span></p>

      <div className="system-actions"><button className="tb" disabled={state.check === "checking" || state.installing} onClick={() => void system.check(true)}><Highlight text={tr("updates.checkNow")} query={query} /></button>
        <span className="muted" role="status">{tr("updates.lastCheck", { time: settings.lastUpdateCheck ? new Date(settings.lastUpdateCheck).toLocaleString(language) : tr("updates.never") })} · {tr(`updates.${state.check}`)}</span></div>
      {state.error && <p role="alert" className="field-error">{state.error}</p>}
    </section>
  </>;
}

function StartMenuPrompt() {
  const { system } = useSystem();
  const tr = useT();
  const [never, setNever] = useState(false);
  const { busy, error, run } = useAction();
  const answer = (install: boolean) => { if (!busy) void run(() => system.answerPrompt(install, never)); };
  return <Modal title={tr("system.promptTitle")} closeDisabled={busy} onClose={() => answer(false)} footer={<>
    <button className="tb" disabled={busy} data-autofocus onClick={() => answer(false)}>{tr("system.noThanks")}</button>
    <button className="tb primary" disabled={busy} onClick={() => answer(true)}>{tr("system.add")}</button>
  </>}>
    <p className="muted">{tr("system.promptHelp")}</p>
    <label className="system-row"><input type="checkbox" checked={never} disabled={busy} onChange={e => setNever(e.target.checked)} />{tr("system.never")}</label>
    {error && <p role="alert" className="field-error">{error}</p>}
  </Modal>;
}

/** Asked once; closing it any way counts as "not now" (off) and it does not return. */
function AutostartPrompt() {
  const { system } = useSystem();
  const tr = useT();
  const { busy, error, run } = useAction();
  const answer = (mode: AutostartMode) => { if (!busy) void run(() => system.answerAutostart(mode)); };
  return <Modal title={tr("system.autostartPromptTitle")} closeDisabled={busy} onClose={() => answer("off")} footer={<>
    <button className="tb" disabled={busy} data-autofocus onClick={() => answer("off")}>{tr("system.autostartLater")}</button>
    <button className="tb" disabled={busy} onClick={() => answer("minimized")}>{tr("system.autostart.minimized")}</button>
    <button className="tb" disabled={busy} onClick={() => answer("shown")}>{tr("system.autostart.shown")}</button>
  </>}>
    <p className="muted">{tr("system.autostartPromptHelp")}</p>
    {error && <p role="alert" className="field-error">{tr("system.autostartFailed", { message: error })}</p>}
  </Modal>;
}

export function SystemUpdates() {
  const { system, state } = useSystem();
  const tr = useT();
  const busyJobs = useAppState(s => s.jobs.jobs.some(job => !isFinished(job.state)));
  const otherDialog = useAppState(s => !!(s.dialog || s.lister || s.compare || s.diff || s.conflicts.length));
  const [notes, setNotes] = useState(false);
  const { controller } = useRuntime();
  const { busy, error, run } = useAction();
  const cancellation = useAction();
  const [manualFailed, setManualFailed] = useState<boolean | null>(null);
  const update = state.status && state.status.packaging !== "flatpak" ? state.update : null;
  const progress = state.progress;
  const progressRef = useRef<HTMLProgressElement>(null);
  useDialogFocus(progressRef, `${progress?.phase}:${state.installed}`);
  const percent = progress?.total ? Math.min(100, Math.floor(100 * progress.downloaded / progress.total)) : undefined;
  const cancel = () => { if (state.installing && !state.installed && progress?.phase === "download") void cancellation.run(system.cancel); };
  // Fehler erscheinen als Toast: das Banner behält seine Höhe und schiebt das Layout nicht.
  const message = error || cancellation.error || state.error;
  useEffect(() => { if (message) controller.toast(message, "error"); }, [message, controller]);
  // Ein-Klick-Update: `system.install` startet nach der Installation selbst neu. Nur wenn das scheitert, bleibt der Knopf.
  const relaunchFailed = state.installed && (manualFailed ?? !!state.error);
  const relaunch = () => {
    setManualFailed(false);
    system.restart().catch((e: unknown) => {
      setManualFailed(true);
      controller.toast(e instanceof Error ? e.message : String(e), "error");
    });
  };
  return <>
    {update && !state.installing && <div className="update-banner" role="region" aria-label={tr("updates.available")}>
      <span><b>{tr("updates.banner", { version: update.version })}</b> {tr(state.status?.appImage ? "updates.signature" : "updates.noAppImage")}</span>
      <button className="tb" onClick={() => setNotes(true)}>{tr("updates.notes")}</button>
      <button className="tb primary" disabled={busyJobs || !state.status || busy} aria-describedby={busyJobs ? "update-jobs" : undefined} onClick={() => void run(system.install)}>{tr(state.status?.appImage ? "updates.install" : "updates.release")}</button>
      <button className="tb" onClick={() => system.dismiss()}>{tr("updates.later")}</button>
      {busyJobs && <p id="update-jobs" className="field-error">{tr("updates.jobs")}</p>}
    </div>}
    {notes && update && <Modal title={`${tr("updates.notes")} · ${update.version}`} onClose={() => setNotes(false)} footer={<button className="tb primary" data-autofocus onClick={() => setNotes(false)}>{tr("action.close")}</button>}>
      <div className="update-notes">{update.notes || tr("updates.noNotes")}</div>
    </Modal>}
    {state.installing && update && <Modal title={tr("updates.progressTitle", { version: update.version })} onClose={cancel} closeDisabled={state.installed || progress?.phase !== "download"} footer={<>
      {state.installed ? (relaunchFailed ? <button className="tb primary" onClick={relaunch}>{tr("updates.restart")}</button> : null) : <button className="tb" disabled={cancellation.busy || progress?.phase !== "download"} onClick={cancel}>{tr("action.cancel")}</button>}
    </>}>
      <p className="muted">{tr("updates.progressHelp")}</p>
      <progress ref={progressRef} className="update-progress" aria-label={tr("updates.downloading")} max={100} value={percent} />
      <p role="status" className="muted">{state.installed ? tr(relaunchFailed ? "updates.restartFailed" : "updates.installed") : progress?.phase === "install" ? tr("updates.installing") : <>{percent !== undefined ? `${percent} % · ` : ""}{tr(progress?.total ? "updates.bytes" : "updates.bytesUnknown", { downloaded: ((progress?.downloaded ?? 0) / 1e6).toFixed(1), total: ((progress?.total ?? 0) / 1e6).toFixed(1) })}</>}</p>
    </Modal>}
    {state.prompt && !otherDialog && !notes && !state.installing && <StartMenuPrompt />}
    {state.autostartPrompt && !state.prompt && !otherDialog && !notes && !state.installing && <AutostartPrompt />}
  </>;
}
