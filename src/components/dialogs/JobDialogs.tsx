import { useState } from "react";
import { useAppState, useController, useT } from "../../app/context";
import type { ItemErrorInfo } from "../../ipc/types";
import { Modal } from "../Modal";

function ItemErrorDialog({ error }: { error: ItemErrorInfo }) {
  const tr = useT();
  const controller = useController();
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const answer = async (decision: "skip" | "retry" | "abort") => {
    setBusy(true);
    await controller.answerItemError(error.jobId, decision, decision === "skip" && remember);
    setBusy(false);
  };
  return <Modal title={tr("itemError.title")} onClose={() => { if (!busy) void answer("abort"); }} footer={<>
    {(["skip", "retry", "abort"] as const).map(decision => <button key={decision} type="button" className="tb" disabled={busy} data-autofocus={decision === "skip" || undefined} title={tr(`itemError.${decision}`)} onClick={() => void answer(decision)}>{tr(`itemError.${decision}`)}</button>)}
  </>}>
    <p>{error.name}</p><p>{tr(`itemError.${error.kind}`)}</p>
    <label><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)} />{tr("itemError.remember")}</label>
  </Modal>;
}

export function JobDialogs() {
  const controller = useController();
  const tr = useT();
  const count = useAppState(s => s.closeRequested);
  const error = useAppState(s => s.itemErrors[0]);
  const [busy, setBusy] = useState(false);
  if (count !== null) return <Modal role="alertdialog" title={tr("closeConfirm.title")} onClose={controller.dismissCloseRequest} footer={<>
    <button type="button" className="tb primary" data-autofocus title={tr("closeConfirm.keep")} onClick={controller.dismissCloseRequest}>{tr("closeConfirm.keep")}</button>
    <button type="button" className="tb" disabled={busy} title={tr("closeConfirm.exit")} onClick={() => { setBusy(true); void controller.appConfirmClose().finally(() => setBusy(false)); }}>{tr("closeConfirm.exit")}</button>
  </>}><p>{tr("closeConfirm.message", { count })}</p></Modal>;
  return error ? <ItemErrorDialog key={error.jobId} error={error} /> : null;
}
