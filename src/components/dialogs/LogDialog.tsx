import { Fragment, useEffect, useRef, useState } from "react";
import { activeTabOf } from "../../app/state";
import { useAppState, useController, useRuntime, useT } from "../../app/context";
import { isArchivePath, parseRemotePath } from "../../state/paths";
import { Modal } from "../Modal";

/** Platzhalter der Anonymisierung (`<server-1>` …) und das gekürzte Home (`~/`) werden grün markiert. */
const REPLACED = /(<(?:benutzer|rechner|email|server-\d+)>|~(?=\/))/g;

function highlight(text: string, on: boolean) {
  if (!on) return text;
  return text.split(REPLACED).map((part, i) => (i % 2 === 1 ? <mark key={i}>{part}</mark> : <Fragment key={i}>{part}</Fragment>));
}

export function LogDialog() {
  const { api } = useRuntime();
  const controller = useController();
  const tr = useT();
  const [anonymize, setAnonymize] = useState(true);
  const [text, setText] = useState("");
  const [note, setNote] = useState("");
  const [error, setError] = useState("");
  const viewer = useRef<HTMLDivElement>(null);
  // Nur lokale Ordner kommen als Speicherziel in Frage; Server, Archive und Ähnliches fallen auf das Home zurück.
  const dir = useAppState(state => {
    const path = activeTabOf(state, state.active).path;
    return parseRemotePath(path) || isArchivePath(path) || !path.startsWith("/") ? state.home : path;
  });

  useEffect(() => {
    let current = true;
    api.logRead(anonymize).then(
      value => { if (current) { setText(value); setError(""); } },
      e => { if (current) setError(e instanceof Error ? e.message : String(e)); },
    );
    return () => { current = false; };
  }, [api, anonymize]);
  // Die neuesten Einträge stehen unten; dort beginnt die Ansicht.
  useEffect(() => {
    const element = viewer.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [text]);

  const copy = async () => {
    setNote(""); setError("");
    try {
      await navigator.clipboard.writeText(text);
      setNote(tr("log.copied"));
    } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
  };
  const save = async () => {
    setNote(""); setError("");
    try {
      setNote(tr("log.saved", { path: await api.logSave(dir, anonymize) }));
    } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
  };

  return <Modal title={tr("log.title")} wide onClose={() => controller.closeDialog()} footer={<>
    {note && <span className="log-note" role="status">{note}</span>}
    <button className="tb" onClick={() => void save()}>{tr("log.save")}</button>
    <button className="tb primary" data-autofocus onClick={() => void copy()}>{tr("log.copy")}</button>
  </>}>
    <label className="system-row"><input type="checkbox" checked={anonymize} onChange={e => setAnonymize(e.target.checked)} /><span>{tr("log.anonymize")}<small>{tr("log.anonymizeHelp")}</small></span></label>
    <div className="log-viewer" ref={viewer} tabIndex={0} role="region" aria-label={tr("log.content")}>{highlight(text, anonymize)}</div>
    <p className="muted log-legend">{tr("log.legend")}{anonymize && <> · <mark>{tr("log.replaced")}</mark> = {tr("log.replacedHint")}</>}</p>
    {error && <p role="alert" className="field-error">{error}</p>}
  </Modal>;
}
