import { useEffect, useState } from "react";
import { useAppState, useController, useT } from "../../app/context";
import type { TranslationKey } from "../../i18n";
import { effectiveBindings, eventToSpec, REBINDABLE, REBINDABLE_COMMANDS, RESERVED_SPECS, type RebindableCommand } from "../../state/keymap";
import { Highlight } from "../../settings/Highlight";
import { normalizeSearch } from "../../settings/catalog";
import { Shortcut } from "../Shortcut";

type Notice = { kind: "reserved" } | { kind: "conflict"; command: RebindableCommand; spec: string; owner: RebindableCommand };

const cmdKey = (command: RebindableCommand) => `keys.cmd.${command}` as TranslationKey;

/** Wirksame Taste eines Befehls: Override, sonst Standard. */
export function effectiveSpec(command: RebindableCommand, bindings: Record<string, string> | undefined): string {
  return effectiveBindings(bindings)[command];
}

/** Abschnitt „Tastenkürzel“ der Einstellungen: Tastenfang, Konfliktprüfung, Zurücksetzen. */
export function KeybindingsSettings({ commands = REBINDABLE_COMMANDS, query = "" }: { commands?: readonly RebindableCommand[]; query?: string }) {
  const controller = useController();
  const tr = useT();
  const bindings = useAppState((state) => state.settings.keybindings);
  const [capturing, setCapturing] = useState<RebindableCommand | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);

  const save = (next: Record<string, string>) => controller.updateSettings((s) => ({ ...s, keybindings: next }));
  /** Speichert nur Abweichungen vom Standard. */
  function assign(command: RebindableCommand, spec: string, base: Record<string, string> = bindings ?? {}): Record<string, string> {
    const next = { ...base };
    if (spec === REBINDABLE[command]) delete next[command];
    else next[command] = spec;
    return next;
  }
  const owner = (spec: string, except: RebindableCommand) => REBINDABLE_COMMANDS.find((c) => c !== except && effectiveSpec(c, bindings) === spec);

  useEffect(() => {
    if (capturing === null) return;
    const command = capturing;
    function onKey(event: KeyboardEvent): void {
      if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) return; // erst die eigentliche Taste zählt
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setCapturing(null);
        setNotice(null);
        return;
      }
      const spec = eventToSpec(event);
      if (spec === null || RESERVED_SPECS.has(spec)) {
        setNotice({ kind: "reserved" });
        return;
      }
      const other = owner(spec, command);
      setCapturing(null);
      if (other) {
        setNotice({ kind: "conflict", command, spec, owner: other });
        return;
      }
      setNotice(null);
      save(assign(command, spec));
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  function replace(n: Extract<Notice, { kind: "conflict" }>): void {
    // Grenze: Der alte Inhaber fällt auf seine Standardtaste zurück; kollidiert diese wiederum
    // mit einem anderen Override, wird das nicht weiter aufgelöst (Doppelbelegung: letzter gewinnt).
    const base = { ...(bindings ?? {}) };
    delete base[n.owner];
    save(assign(n.command, n.spec, base));
    setNotice(null);
  }

  return (
    <div className="keys-group" role="group" aria-label={tr("settings.keys.title")}>
      <h3>{tr("settings.keys.title")}</h3>
      <p className="field-help">{tr("settings.keys.help")}</p>
      <div className="keys-list">
        {commands.map((command) => {
          const spec = effectiveSpec(command, bindings);
          const custom = spec !== REBINDABLE[command];
          const active = capturing === command;
          return (
            <div className="keys-row" key={command}>
              <span className="keys-name"><Highlight text={tr(cmdKey(command))} query={query} /></span>
              {active
                ? <kbd className="keys-key capturing">{tr("settings.keys.capture")}</kbd>
                : <span className={`keys-caps${custom ? " custom" : ""}`}>{query.trim() && normalizeSearch(spec).includes(normalizeSearch(query.trim())) ? <mark><Shortcut spec={spec} /></mark> : <Shortcut spec={spec} />}</span>}
              <span className="keys-actions">
                <button type="button" className="tb" onClick={() => { setNotice(null); setCapturing(active ? null : command); }}>{tr("settings.keys.change")}</button>
                <button type="button" className="tb" disabled={!custom} onClick={() => { const next = { ...(bindings ?? {}) }; delete next[command]; save(next); setNotice(null); }}>{tr("settings.keys.reset")}</button>
              </span>
              {notice !== null && (notice.kind === "reserved" ? active : notice.command === command) ? (
                <div className="keys-msg" role="alert">
                  {notice.kind === "reserved" ? (
                    <span>{tr("settings.keys.reserved")}</span>
                  ) : (
                    <>
                      <span>{tr("settings.keys.conflict", { key: notice.spec, command: tr(cmdKey(notice.owner)) })} {tr("settings.keys.replaceHint", { command: tr(cmdKey(notice.owner)) })}</span>
                      <button type="button" className="tb" onClick={() => replace(notice)}>{tr("settings.keys.replace")}</button>
                      <button type="button" className="tb" onClick={() => setNotice(null)}>{tr("action.cancel")}</button>
                    </>
                  )}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
      <div className="keys-foot">
        <button type="button" className="tb" disabled={!bindings || Object.keys(bindings).length === 0} onClick={() => { save({}); setNotice(null); setCapturing(null); }}>{tr("settings.keys.resetAll")}</button>
      </div>
    </div>
  );
}
