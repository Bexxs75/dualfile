import { useEffect, useState } from "react";
import { useT } from "../../app/context";
import { CoreError, type DualfileApi } from "../../ipc/api";
import type { ConnectionProfile, SshResolvedHost } from "../../ipc/types";
import { errorKey } from "../../i18n";

export const newSshProfile = (host = ""): ConnectionProfile => ({
  id: "", name: host, protocol: "sftp", transport: "openssh", host, port: 0, user: "",
  startFolder: null, tls: "none", auth: { kind: "password" }, allowPlaintext: false, saveSecret: false,
});

export function ConnectionProfileForm({ api, initial, onSaved, onCancel, onImport }: {
  api: DualfileApi; initial: ConnectionProfile; onSaved: () => void; onCancel: () => void; onImport?: (draft: ConnectionProfile) => void;
}) {
  const tr = useT();
  const [profile, setProfile] = useState(initial);
  const [hosts, setHosts] = useState<string[]>([]);
  const [resolved, setResolved] = useState<SshResolvedHost | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const openssh = profile.protocol === "sftp" && profile.transport === "openssh";
  const patch = (value: Partial<ConnectionProfile>) => setProfile((p) => ({ ...p, ...value }));
  useEffect(() => {
    let active = true;
    if (openssh) void api.sshHostsList().then((hosts) => { if (active) setHosts(hosts); }).catch(() => { if (active) setError(tr("ssh.configError")); });
    return () => { active = false; };
  }, [api, openssh]);
  useEffect(() => {
    let active = true;
    setResolved(null);
    if (!openssh || !profile.host) return;
    const timer = setTimeout(() => {
      void api.sshHostResolve(profile.host).then((value) => { if (active) setResolved(value); })
        .catch(() => { if (active) setError(tr("ssh.configError")); });
    }, 250);
    return () => { active = false; clearTimeout(timer); };
  }, [api, openssh, profile.host]);
  if (profile.protocol === "system") return <form className="ssh-profile" onSubmit={event => {
    event.preventDefault(); if (busy) return; setBusy(true); setError("");
    void api.connectionSave(profile).then(onSaved).catch((e: unknown) => setError(tr(errorKey(e instanceof CoreError ? e.kind : "unknown")))).finally(() => setBusy(false));
  }}>
    <p>{tr("system.type")}</p>
    <label>{tr("ssh.name")}<input data-autofocus required maxLength={100} value={profile.name} onChange={e => patch({ name: e.target.value })} /></label>
    <p><code>{profile.systemUrl}</code></p>
    {error && <p role="alert">{error}</p>}
    <div className="ssh-actions"><button type="button" onClick={onCancel}>{tr("action.cancel")}</button><button disabled={busy}>{tr("ssh.save")}</button></div>
  </form>;
  return <form className="ssh-profile" onSubmit={(event) => {
    event.preventDefault(); if (busy) return; setBusy(true); setError("");
    void api.connectionSave(profile).then(onSaved).catch((e: unknown) => setError(tr(errorKey(e instanceof CoreError ? e.kind : "unknown")))).finally(() => setBusy(false));
  }}>
    <label>{tr("ssh.name")}<input data-autofocus required value={profile.name} onChange={(e) => patch({ name: e.target.value })} /></label>
    <label>{tr("ssh.protocol")}<select value={profile.protocol} onChange={(e) => {
      const protocol = e.target.value as ConnectionProfile["protocol"];
      patch({ protocol, transport: "builtin", tls: protocol === "ftps" ? "explicit" : "none", port: protocol === "smb" ? 445 : protocol === "sftp" ? 22 : 21, auth: { kind: "password" }, allowPlaintext: false });
    }}><option value="smb">SMB</option><option value="sftp">SFTP</option><option value="ftps">FTPS</option><option value="ftp">FTP</option></select></label>
    {profile.protocol === "sftp" && <>
      <label>{tr("ssh.transport")}<select value={profile.transport ?? "builtin"} onChange={(e) => patch({ transport: e.target.value as "builtin" | "openssh", saveSecret: false, port: e.target.value === "openssh" ? 0 : profile.port || 22 })}>
        <option value="openssh">{tr("ssh.openssh")}</option><option value="builtin">{tr("ssh.builtin")}</option>
      </select></label>
      {openssh ? <><div className="ssh-note ssh-info">{tr("ssh.opensshHint")}
        <p className="ssh-resolved">{resolved ? tr("ssh.resolved", { host: resolved.hostname, user: resolved.user, port: resolved.port, key: resolved.identityfile.join(", ") }) : "\u00a0"}</p>
      </div><label>{tr("ssh.hostList")}<select value={hosts.includes(profile.host) ? profile.host : ""} onChange={(e) => patch({ host: e.target.value })}>
        <option value="">{tr("ssh.chooseHost")}</option>{hosts.map((host) => <option key={host}>{host}</option>)}
      </select></label>
      {onImport && <button type="button" onClick={() => onImport(profile)}>{tr("ssh.import")}</button>}</> : <div className="ssh-note ssh-warning">{tr("ssh.builtinHint")}</div>}
    </>}
    <label>{tr(profile.protocol === "sftp" ? "ssh.host" : "ssh.server")}<input required value={profile.host} onChange={(e) => patch({ host: e.target.value })} /></label>
    <label>{tr("ssh.user")}<input required={!openssh && !(profile.protocol === "smb" && profile.guest)} value={profile.user} placeholder={openssh ? tr("ssh.inherit") : ""} onChange={(e) => patch({ user: e.target.value })} /></label>
    <label>{tr("ssh.port")}<input type="number" min={1} max={65535} required={!openssh} value={profile.port || ""} placeholder={openssh ? tr("ssh.inherit") : ""} onChange={(e) => patch({ port: Number(e.target.value) })} /></label>
    {profile.protocol === "sftp" && <label>{tr("ssh.key")}<input value={profile.auth.kind === "keyFile" ? profile.auth.path : ""} placeholder={openssh ? tr("ssh.inherit") : ""} onChange={(e) => patch({ auth: e.target.value ? { kind: "keyFile", path: e.target.value } : { kind: "password" } })} /></label>}
    {profile.protocol === "smb" && <>
      <label>{tr("smb.share")}<input value={profile.share ?? ""} onChange={e => patch({ share: e.target.value })} /></label>
      <label>{tr("smb.domain")}<input value={profile.domain ?? ""} onChange={e => patch({ domain: e.target.value })} /></label>
      <label className="ssh-check"><input type="checkbox" checked={profile.guest ?? false} onChange={e => patch({ guest: e.target.checked, saveSecret: false })} />{tr("smb.guest")}</label>
    </>}
    <label>{tr("ssh.folder")}<input value={profile.startFolder ?? ""} onChange={(e) => patch({ startFolder: e.target.value || null })} /></label>
    {profile.protocol === "ftps" && <label>TLS<select value={profile.tls} onChange={(e) => patch({ tls: e.target.value as "explicit" | "implicit" })}><option value="explicit">{tr("ssh.explicit")}</option><option value="implicit">{tr("ssh.implicit")}</option></select></label>}
    {profile.protocol === "ftp" && <label className="ssh-check ssh-note ssh-warning"><input type="checkbox" checked={profile.allowPlaintext} onChange={(e) => patch({ allowPlaintext: e.target.checked })} />{tr("ssh.plaintext")}</label>}
    {!openssh && <label className="ssh-check"><input type="checkbox" checked={profile.saveSecret} onChange={(e) => patch({ saveSecret: e.target.checked })} />{tr("ssh.saveSecret")}</label>}
    {error && <p role="alert">{error}</p>}
    <div className="ssh-actions"><button type="button" onClick={onCancel}>{tr("action.cancel")}</button><button disabled={busy}>{tr("ssh.save")}</button></div>
  </form>;
}
