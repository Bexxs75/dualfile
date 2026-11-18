import { useEffect, useRef, useState } from "react";
import { useAppState, useController, useT } from "../../app/context";
import { CoreError, type DualfileApi } from "../../ipc/api";
import type { ConnectionProfile, ConnectOutcome, SystemPlace } from "../../ipc/types";
import { errorKey } from "../../i18n";
import { Modal } from "../Modal";
import { newSshProfile } from "./ConnectionProfileForm";
import "./ssh.css";
import { SystemPlacesDialog } from "./SystemPlacesDialog";

export function ConnectionsDialog({ api, onClose, entry }: { api: DualfileApi; onClose: () => void; entry?: { action: "import" | "system" | "connect" | "disconnect"; profile?: ConnectionProfile; onConnected?: (root: string) => Promise<boolean> } }) {
  const tr = useT();
  const controller = useController();
  const active = useAppState((s) => s.active);
  const [places, setPlaces] = useState<SystemPlace[]>([]);
  const [showSystem, setShowSystem] = useState(false);
  const entries = useAppState((s) => s.connections);
  // Comparing sub-views against the list only makes sense once the list was fetched at least once.
  const [synced, setSynced] = useState(false);
  const [hosts, setHosts] = useState<string[] | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [password, setPassword] = useState<ConnectionProfile | null>(null);
  const [trust, setTrust] = useState<{ profile: ConnectionProfile; outcome: Exclude<ConnectOutcome, { state: "connected" }> } | null>(null);
  const [deleting, setDeleting] = useState<ConnectionProfile | null>(null);
  const [attemptError, setAttemptError] = useState<{ id: string; text: string } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const errorText = (e: unknown) => e instanceof CoreError && (e.kind === "systemMountFailed" || e.kind === "systemMountTimeout")
    ? tr(e.kind === "systemMountTimeout" ? "error.systemMountTimeout" : "error.systemMountFailed", { detail: e.message || tr("error.systemMountNoDetails") })
    : tr(errorKey(e instanceof CoreError ? e.kind : "unknown"));
  const fail = (e: unknown) => setError(errorText(e));
  const failAttempt = async (profile: ConnectionProfile, e: unknown) => {
    let host = profile.host;
    let port = profile.port || (profile.protocol === "smb" ? 445 : profile.protocol === "nfs" ? 2049 : profile.protocol === "sftp" ? 22 : 21);
    if (profile.transport === "openssh") {
      try {
        const resolved = await api.sshHostResolve(profile.host);
        host = resolved.hostname;
        port = profile.port || resolved.port;
      } catch { /* Keep the profile target if SSH configuration cannot be resolved. */ }
    }
    if (profile.protocol === "system") {
      const scheme = profile.systemUrl?.split(":")[0] ?? "";
      setAttemptError({ id: profile.id, text: `${scheme}:// · ${errorText(e)}` }); return;
    }
    setAttemptError({ id: profile.id, text: `${tr(errorKey(e instanceof CoreError ? e.kind : "unknown")).replace(/[.。]$/, "")}: ${host.includes(":") ? `[${host}]` : host}:${port}` });
  };
  const edit = (profile?: ConnectionProfile) => { setAttemptError(null); setError(""); void controller.openConnectionEditor(profile); };
  const refresh = () => controller.refreshConnections();
  useEffect(() => { void refresh().catch(fail).finally(() => setSynced(true)); void api.systemPlacesList().then(setPlaces).catch(fail); }, [api]);
  // The editor window can change a profile while a password or host-key question for its old target is open.
  useEffect(() => {
    if (!synced) return;
    const changed = (shown: ConnectionProfile) => JSON.stringify(entries.find((e) => e.profile.id === shown.id)?.profile) !== JSON.stringify(shown);
    if (password && changed(password)) { setPassword(null); controller.toast(tr("ssh.profileChanged"), "error"); }
    if (trust && changed(trust.profile)) { setTrust(null); controller.toast(tr("ssh.profileChanged"), "error"); }
  }, [entries, synced, password, trust]);
  async function outcome(profile: ConnectionProfile, result: ConnectOutcome) {
    if (result.state === "connected") {
      await controller.refreshConnections();
      if (entry?.onConnected) await entry.onConnected(result.root);
      else await controller.navigateTo(active, result.root);
      onClose();
    } else setTrust({ profile, outcome: result });
  }
  async function connect(profile: ConnectionProfile, secret?: string) {
    setPassword(null); setBusy(true); setError(""); setAttemptError(null);
    try {
      if (profile.protocol === "system") {
        const root = await api.systemMountConnect(profile.id);
        if (await (entry?.onConnected ? entry.onConnected(root) : controller.navigateTo(active, root))) onClose(); return;
      }
      await outcome(profile, await api.connectionConnect(profile.id, secret, profile)); } catch (e) { await failAttempt(profile, e); }
    finally { await refresh(); setBusy(false); }
  }
  const openSystem = () => api.systemPlacesList().then(p => { setPlaces(p); setShowSystem(true); }).catch(fail);
  const disconnect = async (profile: ConnectionProfile) => {
    setBusy(true);
    try {
      if (profile.protocol === "system") return;
      await api.connectionDisconnect(profile.id);
      await refresh();
    } catch (e) { fail(e); } finally { setBusy(false); }
  };
  const entered = useRef(false);
  useEffect(() => {
    if (entered.current || !entry) return;
    entered.current = true;
    if (entry.action === "import") void api.sshHostsList().then(setHosts).catch(fail);
    else if (entry.action === "system") void openSystem();
    else if (entry.profile) {
      if (entry.action === "disconnect") void disconnect(entry.profile);
      else if (entry.profile.protocol === "system" || entry.profile.transport === "openssh" || (entry.profile.protocol === "smb" && entry.profile.guest) || entry.profile.protocol === "nfs") void connect(entry.profile);
      else setPassword(entry.profile);
    }
  }, [entry]);
  if (showSystem) return <SystemPlacesDialog api={api} places={places} profiles={entries.map(e => e.profile)} onClose={() => setShowSystem(false)} onImported={async () => { await refresh(); setShowSystem(false); }} />;
  if (deleting) return <Modal title={tr("ssh.delete")} onClose={() => { if (!busy) { setDeleting(null); setError(""); } }} footer={<>
    <button className="tb" disabled={busy} onClick={() => { setDeleting(null); setError(""); }}>{tr("action.cancel")}</button>
    <button className="tb warn" disabled={busy} onClick={() => {
      setBusy(true); setError("");
      void api.connectionDelete(deleting.id).then(async () => { await refresh(); setDeleting(null); })
        .catch(fail).finally(() => setBusy(false));
    }}>{tr("ssh.delete")}</button>
  </>}><p>{tr("ssh.deleteConfirm", { name: deleting.name })}</p>{error && <p role="alert">{error}</p>}</Modal>;
  if (password) return <Modal title={tr("ssh.prompt.password")} onClose={() => setPassword(null)}>
    <form autoComplete="off" onSubmit={(event) => { event.preventDefault(); const secret = input.current?.value ?? ""; if (input.current) input.current.value = ""; void connect(password, secret); }}>
      <label>{tr("ssh.prompt.password")}<input ref={input} data-autofocus type="password" autoComplete="off" /></label>
      <button className="tb" type="button" onClick={() => setPassword(null)}>{tr("action.cancel")}</button><button className="tb">{tr("ssh.connect")}</button>
    </form>
  </Modal>;
  if (trust) return <Modal title={tr("ssh.prompt.hostKey")} onClose={() => setTrust(null)} footer={<>
    <button className="tb" onClick={() => setTrust(null)}>{tr("action.cancel")}</button>
    {trust.outcome.state === "needsTrust" && <button className="tb" disabled={busy} onClick={() => {
      if (trust.outcome.state !== "needsTrust") return;
      setBusy(true);
      void api.connectionTrust(trust.profile.id, "once", trust.outcome.fingerprint)
        .then((result) => outcome(trust.profile, result)).catch(async (e: unknown) => { const profile = trust.profile; setTrust(null); await failAttempt(profile, e); }).finally(() => setBusy(false));
    }}>{tr("ssh.trustOnce")}</button>}
  </>}>
    <p>{tr(trust.outcome.state === "pinMismatch" ? "error.pinMismatch" : "ssh.verifyKey")}</p>
    <code style={{ overflowWrap: "anywhere" }}>{trust.outcome.state === "needsTrust" ? trust.outcome.fingerprint : `${trust.outcome.old} → ${trust.outcome.new}`}</code>
    {error && <p role="alert">{error}</p>}
  </Modal>;
  if (hosts) return <Modal title={tr("ssh.import")} onClose={() => { if (!busy) setHosts(null); }} footer={<>
    <button className="tb" disabled={busy} onClick={() => setHosts(null)}>{tr("action.cancel")}</button>
    <button className="tb" disabled={busy || selected.length === 0} onClick={() => {
      setBusy(true); setError("");
      void (async () => {
        for (const host of selected) {
          await api.connectionSave(newSshProfile(host));
          // A partial failure must not offer already imported hosts again.
          setSelected((items) => items.filter((item) => item !== host));
          setHosts((items) => items?.filter((item) => item !== host) ?? null);
        }
        setHosts(null); await refresh();
      })().catch(fail).finally(() => setBusy(false));
    }}>{tr("ssh.importSelected")}</button>
  </>}>
    {hosts.length === 0 && <p>{tr("ssh.noHosts")}</p>}
    {hosts.map((host) => <label className="ssh-host" key={host}><input type="checkbox" disabled={busy} checked={selected.includes(host)} onChange={(e) => setSelected((items) => e.target.checked ? [...items, host] : items.filter((item) => item !== host))} />{host}</label>)}
    {error && <p role="alert">{error}</p>}
  </Modal>;
  return <Modal title={tr("ssh.connections")} onClose={onClose} footer={<>
    <button className="tb" disabled={busy} onClick={() => edit()}>{tr("ssh.new")}</button>
    <button className="tb" disabled={busy} onClick={() => { setSelected([]); setError(""); void api.sshHostsList().then(setHosts).catch(fail); }}>{tr("ssh.import")}</button>
    <button className="tb" disabled={busy} onClick={() => { setError(""); void openSystem(); }}>{tr("system.import")}</button>
    <button className="tb" onClick={onClose}>{tr("action.close")}</button>
  </>}>
    {entries.length === 0 && places.length > 0 && <p>{tr("system.hint")}</p>}
    {entries.map(({ profile, connected }) => <div className="ssh-entry" key={profile.id}>
      <strong>{profile.name}</strong><span>{profile.protocol === "system" ? tr("system.type") : profile.protocol.toUpperCase()} · {profile.host}</span>
      <button className="tb" disabled={busy} onClick={() => edit(profile)}>{tr("ssh.edit")}</button>
      {connected && profile.protocol !== "system" ? <button className="tb" disabled={busy} onClick={() => { void disconnect(profile); }}>{tr("ssh.disconnect")}</button>
        : <button className="tb" disabled={busy} onClick={() => { if (profile.protocol === "system" || profile.transport === "openssh" || (profile.protocol === "smb" && profile.guest) || profile.protocol === "nfs") void connect(profile); else { setAttemptError(null); setPassword(profile); } }}>{tr("ssh.connect")}</button>}
      {profile.protocol === "system" && connected && <>
        <button className="tb" disabled={busy} onClick={() => { setBusy(true); void controller.unmountPlace(`conn:${profile.id}`).then(refresh).catch(fail).finally(() => setBusy(false)); }}>{tr("system.unmount")}</button>
      </>}
      <button className="tb" disabled={busy} onClick={() => { setAttemptError(null); setError(""); setDeleting(profile); }}>{tr("ssh.delete")}</button>
      {attemptError?.id === profile.id && <p className="ssh-attempt-error" role="alert">{attemptError.text}</p>}
    </div>)}
    {busy && <p role="status">{tr("ssh.connecting")}</p>}
    {error && <p role="alert">{error}</p>}
  </Modal>;
}
