import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useAppState, useLanguage, useRuntime, useT } from "../../app/context";
import { isFinished } from "../../state/operations";
import { useDialogFocus } from "./useDialogFocus";
import { Modal } from "../Modal";
import config from "../../../src-tauri/tauri.conf.json";

function useSystem() {
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

export function SystemSettings() {
  const { system, state } = useSystem();
  const { controller } = useRuntime();
  const settings = useAppState(s => s.settings);
  const language = useLanguage();
  const tr = useT();
  const { busy, error, run } = useAction();
  return <>
    <section className="system-group" aria-labelledby="system-heading">
      <h3 id="system-heading">{tr("system.settingsTitle")}</h3>
      <label className="system-row"><input type="checkbox" checked={settings.trayEnabled ?? false} onChange={e => {
        const enabled = e.target.checked;
        controller.updateSettings(s => ({ ...s, trayEnabled: enabled }));
        void system.setTray(enabled, language);
      }} /><span>{tr("system.tray")}<small>{tr("system.trayHelp")}</small></span></label>
      {state.trayUnavailable && <p className="field-error" role="status">{tr("system.trayUnavailable")}</p>}
      <p className="muted">{tr("system.startMenu")} <b className={state.status?.startMenuExists ? "system-present" : undefined}>{tr(!state.status ? "system.loading" : state.status.startMenuExists ? "system.present" : "system.absent")}</b></p>
      <div><button className="tb" disabled={busy || !state.status} onClick={() => void run(() => system.setStartMenu(!state.status?.startMenuExists))}>{tr(state.status?.startMenuExists ? "system.remove" : "system.add")}</button></div>
      {error && <p role="alert" className="field-error">{error}</p>}
    </section>
  </>;
}

const REPO_URL = "https://github.com/Bexxs75/dualfile";

export function AboutSettings() {
  const { api } = useRuntime();
  const tr = useT();
  const { error, run } = useAction();
  return <section className="system-group" aria-labelledby="about-heading">
    <h3 id="about-heading">{tr("settings.about.heading")}</h3>
    <p>Dualfile <span className="system-version">{config.version}</span> · {tr("settings.about.license")}</p>
    <div><button className="tb" onClick={() => void run(() => api.openUrl(REPO_URL))}>{tr("settings.about.github")}</button></div>
    {error && <p role="alert" className="field-error">{error}</p>}
  </section>;
}

function formatBytes(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function TroubleshootSettings() {
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
    <h3 id="troubleshoot-heading">{tr("log.heading")}</h3>
    <p>{tr("log.intro")}</p>
    {info && <p><span className="muted">{tr("log.location")}</span> <code>{info.path}</code> <span className="muted">{formatBytes(info.bytes)}</span></p>}
    <div className="system-actions">
      <button className="tb primary" onClick={() => controller.openDialog({ kind: "log", fromSettings: true })}>{tr("log.view")}</button>
      <button className="tb" onClick={() => void run(() => api.logOpenFolder())}>{tr("log.openFolder")}</button>
      <button className="tb" onClick={() => void run(async () => api.openUrl(await api.reportIssueUrl()))}>{tr("log.report")}</button>
    </div>
    <p className="muted">{tr("log.reportHelp")}</p>
    {error && <p role="alert" className="field-error">{error}</p>}
  </section>;
}

export function UpdateSettings() {
  const { system, state } = useSystem();
  const { controller } = useRuntime();
  const settings = useAppState(s => s.settings);
  const language = useLanguage();
  const tr = useT();
  return <>
    <section className="system-group" aria-labelledby="updates-heading">
      <h3 id="updates-heading">{tr("updates.title")}</h3>
      <p>{tr("updates.version")} <span className="system-version">{config.version}</span></p>
      <label className="system-row"><input type="checkbox" checked={settings.checkUpdatesOnStart !== false} onChange={e => controller.updateSettings(s => ({ ...s, checkUpdatesOnStart: e.target.checked }))} /><span>{tr("updates.onStart")}<small>{tr("updates.onStartHelp")}</small></span></label>
      <div className="system-actions"><button className="tb" disabled={state.check === "checking" || state.installing} onClick={() => void system.check(true)}>{tr("updates.checkNow")}</button>
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
  const update = state.update;
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
  </>;
}
