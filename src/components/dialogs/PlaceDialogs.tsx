import { useState } from "react";
import { useAppState, useController, useT } from "../../app/context";
import type { Dialog } from "../../app/state";
import { buildPlaces } from "../../state/places";
import { Modal } from "../Modal";

export function PlaceConfirmDialog({ dialog }: { dialog: Extract<Dialog, { kind: "placeConfirm" }> }) {
  const controller = useController(), tr = useT();
  const title = dialog.action === "mount" ? "sidebar.mount" : dialog.action === "unmount" ? "sidebar.unmount" : "sidebar.emptyTrash";
  const warning = dialog.action === "unmount" ? "sidebar.unmountWarning" : "sidebar.emptyTrashWarning";
  return <Modal role="alertdialog" title={tr(title)} onClose={() => controller.closeDialog()} closeDisabled={dialog.busy} footer={<>
    <button disabled={dialog.busy} className="tb primary" data-autofocus onClick={() => controller.closeDialog()}>{tr("action.cancel")}</button>
    <button disabled={dialog.busy} className="tb warn" onClick={() => void controller.confirmPlaceAction()}>{tr("sidebar.confirm")}</button>
  </>}><p role={dialog.busy ? "status" : undefined}>{tr(dialog.busy ? dialog.action === "mount" ? "sidebar.mountBusy" : "sidebar.unmountBusy" : warning)}</p></Modal>;
}

export function PlacePropertiesDialog({ placeKey }: { placeKey: string }) {
  const state = useAppState(s => s), controller = useController(), tr = useT();
  const place = buildPlaces(state.places, state.connections, state.settings, state.home, true).flatMap(s => s.entries).find(e => e.key === placeKey);
  const drive = state.places?.drives.find(d => d.path === place?.path);
  const [space, setSpace] = useState(place?.space);
  const [busy, setBusy] = useState(false);
  const [measured, setMeasured] = useState(false);
  return <Modal title={tr("context.properties")} onClose={() => controller.closeDialog()}>
    <p>{place?.label ? tr(place.label) : place?.name}</p><p>{place?.path}</p>
    {place?.state && <p>{tr(`sidebar.${place.state}`)}</p>}
    {drive && <dl><dt>{tr("sidebar.device")}</dt><dd>{drive.device || "—"}</dd><dt>{tr("sidebar.filesystem")}</dt><dd>{drive.fstype}</dd>
      <dt>{tr("sidebar.readOnly")}</dt><dd>{tr(drive.readOnly ? "sidebar.yes" : "sidebar.no")}</dd>
      {drive.fstab && <><dt>fstab</dt><dd>{[drive.fstab.permission, drive.fstab.automount && "x-systemd.automount", drive.fstab.noauto && "noauto"].filter(Boolean).join(", ") || "—"}</dd></>}
    </dl>}
    {space && <p>{tr("sidebar.space", { free: (space.available / 1e9).toFixed(1), total: (space.total / 1e9).toFixed(1) })}</p>}
    {!space && measured && <p role="status">{tr("sidebar.unknownSpace")}</p>}
    {place?.path && place.state !== "unmounted" && place.state !== "automount" && <button className="tb" disabled={busy} onClick={() => {
      setBusy(true); void controller.api.placesSpace(place.path!).then(setSpace).catch(() => setSpace(null)).finally(() => { setBusy(false); setMeasured(true); });
    }}>{tr("sidebar.measureSpace")}</button>}
  </Modal>;
}
