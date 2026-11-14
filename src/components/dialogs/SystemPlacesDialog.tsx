import { useState } from "react";
import { useT } from "../../app/context";
import { CoreError, type DualfileApi } from "../../ipc/api";
import type { ConnectionProfile, SystemPlace } from "../../ipc/types";
import { systemPlaceExists } from "../../ipc/systemPlaces";
import { errorKey } from "../../i18n";
import { Modal } from "../Modal";

export function SystemPlacesDialog({ api, places, profiles, onClose, onImported }: {
  api: DualfileApi; places: SystemPlace[]; profiles: ConnectionProfile[]; onClose: () => void; onImported: () => Promise<void>;
}) {
  const tr = useT();
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <Modal title={tr("system.title")} onClose={() => { if (!busy) onClose(); }} footer={<>
    <button className="tb" disabled={busy} onClick={onClose}>{tr("action.cancel")}</button>
    <button className="tb" disabled={busy || selected.length === 0} onClick={() => {
      setBusy(true); setError("");
      void api.systemPlacesImport(selected).then(onImported).catch((e: unknown) => setError(tr(errorKey(e instanceof CoreError ? e.kind : "unknown")))).finally(() => setBusy(false));
    }}>{tr("system.take")}</button>
  </>}>
    {places.length === 0 && <p>{tr("system.empty")}</p>}
    {places.map(place => {
      const exists = systemPlaceExists(place, profiles);
      return <label className="ssh-host" key={place.url} style={{ opacity: exists ? 0.55 : 1 }}>
        <input type="checkbox" disabled={busy || exists} checked={selected.includes(place.url)} onChange={e => setSelected(s => e.target.checked ? [...s, place.url] : s.filter(url => url !== place.url))} />
        <span><strong>{place.name}</strong> <code>{place.scheme.toUpperCase()}</code> · {place.host}<br />
          {place.sources.map(source => tr(source === "dolphin" ? "system.dolphin" : source === "gnome" ? "system.gnome" : "system.mounted")).join(" · ")}
          {exists && <> · {tr("system.exists")}</>}
        </span>
      </label>;
    })}
    {places.some(p => p.scheme === "ftp") && <p>{tr("system.ftpWarning")}</p>}
    {error && <p role="alert">{error}</p>}
  </Modal>;
}
