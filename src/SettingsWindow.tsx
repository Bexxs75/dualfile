import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { RuntimeProvider, useAppState, useLanguage, useT } from "./app/context";
import { createRuntime } from "./app/runtime";
import { createSettingsWindowClient, type SizePrompt } from "./app/settingsWindow";
import { parseAppearance } from "./ConnectionEditorWindow";
import { SettingsPanel } from "./components/dialogs/SettingsDialog";
import { LogDialog } from "./components/dialogs/LogDialog";
import { isModalOpen, Modal } from "./components/Modal";
import { applyTheme } from "./themes/tokens";
import type { DualfileApi } from "./ipc/api";
import { resolveSection } from "./settings/catalog";
import type { SettingsTab } from "./ipc/types";

export function parseSettingsParams(search: string) {
  const tab = new URLSearchParams(search).get("tab");
  return { appearance: parseAppearance(search),
    tab: resolveSection(tab) };
}
/** "Keep the window size?" while closing. Enter = yes; Esc closes without changing anything. */
function SizePromptDialog({ prompt }: { prompt: SizePrompt }) {
  const tr = useT();
  const [dontAsk, setDontAsk] = useState(false);
  const params = { width: prompt.width, height: prompt.height };
  return <Modal role="alertdialog" className="size-prompt" title={tr("settings.sizePrompt.title")} onClose={() => prompt.answer({ choice: "dismiss", dontAsk: false })}
    footer={<>
      <button type="button" className="tb" onClick={() => prompt.answer({ choice: "no", dontAsk })}>{tr("settings.sizePrompt.no")}</button>
      <button type="button" className="tb primary" data-autofocus onClick={() => prompt.answer({ choice: "yes", dontAsk })}>{tr("settings.sizePrompt.yes")}</button>
    </>}>
    <div onKeyDown={e => { if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) { e.preventDefault(); prompt.answer({ choice: "yes", dontAsk }); } }}>
      <p>{tr(prompt.direction === "larger" ? "settings.sizePrompt.larger" : "settings.sizePrompt.smaller", params)}</p>
      <label className="size-prompt-check"><input type="checkbox" checked={dontAsk} aria-describedby="size-prompt-help" onChange={e => setDontAsk(e.target.checked)} />{tr("settings.sizePrompt.dontAsk")}</label>
      <p id="size-prompt-help" className="field-help">{tr("settings.sizePrompt.dontAskHelp")}</p>
    </div>
  </Modal>;
}
function Panel({ client, tab }: { client: ReturnType<typeof createSettingsWindowClient>; tab: SettingsTab }) {
  const state = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const settings = useAppState(s => s.settings);
  const dialog = useAppState(s => s.dialog);
  const lang = useLanguage(), tr = useT();
  useEffect(() => { document.documentElement.classList.add("editor-window-root"); return () => document.documentElement.classList.remove("editor-window-root"); }, []);
  useEffect(() => { applyTheme(document.documentElement, settings); document.documentElement.lang = lang; }, [settings, lang]);
  const ready = !!state.snapshot;
  useEffect(() => { if (ready) document.querySelector<HTMLElement>("[data-autofocus]")?.focus(); }, [ready]);
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === "Escape" && !e.defaultPrevented && !isModalOpen()) { e.preventDefault(); void client.close(); } };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, [client]);
  const outcome = state.snapshot?.persistence.outcome;
  return <main className="editor-window settings-window" aria-busy={!ready} aria-label={tr("settings.title")}>
    <div className="editor-scroll"><fieldset disabled={state.closing}>
      {ready && <SettingsPanel startTab={tab} onClose={() => void client.close()} />}
    </fieldset></div>
    <footer className="settings-window-foot">
      {outcome?.kind === "repaired" && <p role="status">{tr("notice.repaired", { messages: outcome.messages.join("; ") })}</p>}
      {outcome?.kind === "recoveredFromCorrupt" && <p role="status">{tr("notice.recoveredFromCorrupt")}</p>}
      {outcome?.kind === "migrated" && <p role="status">{tr("notice.migrated", { from: outcome.from, to: outcome.to })}</p>}
      {outcome?.kind === "newerVersion" && <p role="status">{tr("notice.newerVersion", { version: outcome.version })}</p>}
      {state.snapshot?.persistence.error && <p role="alert">{state.snapshot.persistence.error}</p>}
      {state.error && <p role="alert">{state.error}</p>}
      {state.closing && <p role="status">{tr("settings.applying")}</p>}
      {state.canForce && <div><p>{tr("settings.unsavedLost")}</p><button className="tb" onClick={() => void client.forceClose()}>{tr("settings.closeAnyway")}</button></div>}

    </footer>
    {dialog?.kind === "log" && <LogDialog saveDir={state.snapshot?.logDir} />}
    {state.sizePrompt && <SizePromptDialog prompt={state.sizePrompt} />}
  </main>;
}
export function SettingsWindow({ api, search }: { api: DualfileApi; search: string }) {
  const params = useMemo(() => parseSettingsParams(search), [search]);
  const client = useMemo(() => {
    const rt = createRuntime(api);
    const { lang, ...appearance } = params.appearance;
    rt.controller.updateSettings(s => ({ ...s, ...appearance, language: lang }));
    return createSettingsWindowClient(api, rt);
  }, [api, params]);
  useEffect(() => { void client.attach(); }, [client]);
  return <RuntimeProvider runtime={client.runtime}><Panel client={client} tab={params.tab} /></RuntimeProvider>;
}
