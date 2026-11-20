import { useEffect, useState, type CSSProperties } from "react";
import { useAppState, useController, useT } from "../../app/context";
import type { TranslationKey } from "../../i18n";
import type { Settings } from "../../ipc/types";
import {
  accentPresets,
  accentWarning,
  accentWarningKeys,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  isValidHex,
  themeNames,
  themes,
  defaultAccent,
} from "../../themes/tokens";
import {
  DEFAULT_TERMINAL_BG,
  DEFAULT_TERMINAL_COLORS,
  DEFAULT_TERMINAL_FG,
  MIN_TERMINAL_CONTRAST,
  terminalColorModes,
  terminalColorsFor,
  terminalContrast,
  type TerminalColorMode,
} from "../../themes/terminal";
import { AboutSettings, SystemSettings, TroubleshootSettings, UpdateSettings } from "./SystemDialogs";
import { KeybindingsSettings } from "./KeybindingsSettings";
import { Modal } from "../Modal";

function Segmented<V extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: V;
  options: { value: V; text: string }[];
  onChange: (value: V) => void;
}) {
  return (
    <div className="seg" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={option.value === value ? "on" : undefined}
          onClick={() => onChange(option.value)}
        >
          {option.text}
        </button>
      ))}
    </div>
  );
}

function ColorField({ id, label, hex, onValid }: { id: string; label: string; hex: string; onValid: (hex: string) => void }) {
  const tr = useT();
  const [text, setText] = useState(hex);
  // Follows changes from outside (colour picker, swap button) unless the user is mid-typing an invalid value.
  useEffect(() => setText(hex), [hex]);
  const invalid = !isValidHex(text);
  return (
    <div className="cpick">
      <input type="color" aria-label={tr("settings.terminal.pick", { name: label })} value={hex} onChange={e => onValid(e.target.value.toLowerCase())} />
      <input
        id={id}
        className="hex"
        aria-label={tr("settings.terminal.hex", { name: label })}
        value={text}
        maxLength={7}
        spellCheck={false}
        aria-invalid={invalid}
        onChange={e => {
          const value = e.target.value.trim();
          const prefixed = value && !value.startsWith("#") ? `#${value}` : value;
          setText(prefixed);
          if (isValidHex(prefixed)) onValid(prefixed.toLowerCase());
        }}
      />
      <span>{label}</span>
    </div>
  );
}

function TerminalColorSettings({ set }: { set: (patch: Partial<Settings>) => void }) {
  const tr = useT();
  const settings = useAppState((state) => state.settings);
  const mode: TerminalColorMode = settings.terminalColors ?? DEFAULT_TERMINAL_COLORS;
  const bg = settings.terminalBg ?? DEFAULT_TERMINAL_BG;
  const fg = settings.terminalFg ?? DEFAULT_TERMINAL_FG;
  const colors = terminalColorsFor(settings);
  const ratio = terminalContrast(colors);
  const good = ratio >= MIN_TERMINAL_CONTRAST;
  return (
    <section className="settings-section" aria-labelledby="terminal-colors-title">
      <h3 id="terminal-colors-title">{tr("settings.terminal.title")}</h3>
      <div className="fld tiles-fld">
        <span id="terminal-colors-label">{tr("settings.terminal.colors")}</span>
        <div className="presets" role="group" aria-labelledby="terminal-colors-label">
          {terminalColorModes.map(value => {
            const c = terminalColorsFor({ ...settings, terminalColors: value, terminalBg: bg, terminalFg: fg });
            return (
              <button key={value} type="button" className="preset" aria-pressed={value === mode} onClick={() => set({ terminalColors: value, terminalBg: bg, terminalFg: fg })}>
                <span className="preview" aria-hidden="true" style={{ background: c.bg, color: c.fg }}>$ ls<br />src  docs</span>
                <b>{tr(`settings.terminal.mode.${value}` as TranslationKey)}</b>
              </button>
            );
          })}
        </div>
      </div>
      {mode === "custom" && (
        <div className="fld tiles-fld">
          <span>{tr("settings.terminal.custom")}</span>
          <div className="terminal-custom">
            <ColorField id="terminal-bg" label={tr("settings.terminal.bg")} hex={bg} onValid={terminalBg => set({ terminalBg })} />
            <ColorField id="terminal-fg" label={tr("settings.terminal.fg")} hex={fg} onValid={terminalFg => set({ terminalFg })} />
            <button type="button" className="link" onClick={() => set({ terminalBg: fg, terminalFg: bg })}>{tr("settings.terminal.swap")}</button>
          </div>
        </div>
      )}
      <div className="fld">
        <span />
        <div className="contrast" role="status">
          <span className={good ? "badge ok" : "badge bad"}>{tr("settings.terminal.ratio", { ratio: ratio.toFixed(1) })}</span>
          {tr(good ? "settings.terminal.contrast.ok" : "settings.terminal.contrast.low")}
        </div>
      </div>
      <p className="field-help">{tr(`settings.terminal.help.${mode === "theme" || mode === "custom" ? mode : "preset"}` as TranslationKey)}</p>
    </section>
  );
}

/** Aussehen und Sprache. Änderungen gelten sofort (ohne Neustart) und werden entprellt gespeichert. */
export function SettingsPanel({ startTab }: { startTab?: import("../../ipc/types").SettingsTab }) {
  const controller = useController();
  const tr = useT();
  const settings = useAppState((state) => state.settings);
  const [hex, setHex] = useState(settings.accentAutomatic ? defaultAccent(settings.theme) : settings.accent);
  const set = (patch: Partial<Settings>) => controller.updateSettings((s) => ({ ...s, ...patch }));
  const currentAccent = settings.accentAutomatic ? defaultAccent(settings.theme) : settings.accent;
  const warnings = accentWarning(hex, settings.theme);
  const tabs = ["appearance", "system", "info", "keyboard"] as const;
  useEffect(() => setHex(currentAccent), [currentAccent, settings.theme, settings.accentAutomatic]);
  const [tab, setTab] = useState<(typeof tabs)[number]>(startTab ?? "appearance");

  function onHex(value: string): void {
    setHex(value);
    if (isValidHex(value)) set({ accent: value.toLowerCase(), accentAutomatic: false });
  }

  return (
    <>
      <div className="settings-tabs" role="tablist" aria-label={tr("settings.tabs")}>
        {tabs.map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            id={`settings-tab-${id}`}
            aria-selected={tab === id}
            aria-controls="settings-panel"
            className={tab === id ? "on" : undefined}
            onClick={() => setTab(id)}
          >
            {tr(`settings.tab.${id}` as TranslationKey)}
          </button>
        ))}
      </div>
      <div className="settings" role="tabpanel" id="settings-panel" aria-labelledby={`settings-tab-${tab}`}>
        {tab === "appearance" && <>
        <section className="settings-section" aria-label={tr("sidebar.title")}>
          <h3>{tr("sidebar.title")}</h3>
          <Segmented label={tr("sidebar.mode")} value={settings.sidebarMode ?? "full"} options={(["hidden", "icons", "full"] as const).map(value => ({ value, text: tr(`sidebar.${value}`) }))} onChange={sidebarMode => set({ sidebarMode })} />
          <label className="fld"><span>{tr("sidebar.width")}</span><input type="number" min={160} max={400} value={settings.sidebarWidth ?? 210} onChange={e => { const n = Number(e.target.value); if (n >= 160 && n <= 400) set({ sidebarWidth: Math.round(n) }); }} /></label>
          {(["places", "remote", "devices"] as const).map(id => <label className="fld" key={id}><span>{tr(id === "devices" ? "places.drives" : `sidebar.${id}`)}</span><input type="checkbox" checked={!(settings.sidebarCollapsed ?? []).includes(id)} onChange={e => set({ sidebarCollapsed: e.target.checked ? (settings.sidebarCollapsed ?? []).filter(v => v !== id) : [...(settings.sidebarCollapsed ?? []), id] })} /></label>)}
          <label className="fld"><span>{tr("sidebar.discovery")}</span><input type="checkbox" checked={settings.sidebarDiscovery ?? false} onChange={e => set({ sidebarDiscovery: e.target.checked })} /></label>
          <p className="muted">{tr("sidebar.discoveryHelp")}</p>
          <button className="tb" disabled={!settings.sidebarHidden?.length} onClick={() => set({ sidebarHidden: [] })}>{tr("sidebar.resetHidden")}</button>
          <p className="muted">{tr("sidebar.desktopHelp")}</p>
        </section>
        <label className="fld"><span>{tr("settings.showParentEntry")}</span><input type="checkbox" checked={settings.showParentEntry !== false} onChange={event => set({ showParentEntry: event.target.checked })} /></label>
        <label className="fld"><span>{tr("settings.extensionColumn")}</span><input type="checkbox" checked={settings.extensionColumn === true} onChange={event => set({ extensionColumn: event.target.checked })} /></label>
        <label className="fld"><span>{tr("settings.showPermissionsColumn")}</span><input type="checkbox" checked={settings.showPermissionsColumn === true} onChange={event => set({ showPermissionsColumn: event.target.checked })} /></label>
        <label className="fld"><span>{tr("settings.showUidColumn")}</span><input type="checkbox" checked={settings.showUidColumn === true} onChange={event => set({ showUidColumn: event.target.checked })} /></label>
        <label className="fld"><span>{tr("settings.showGidColumn")}</span><input type="checkbox" checked={settings.showGidColumn === true} onChange={event => set({ showGidColumn: event.target.checked })} /></label>
        <div className="fld"><span>{tr("settings.paneColors.title")}</span></div>
        <div className="fld">
          <label htmlFor="pane-color-left">{tr("settings.paneColors.left")}</label>
          <input id="pane-color-left" type="color" value={settings.paneColorLeft || themes[settings.theme].panel} onChange={e => set({ paneColorLeft: e.target.value })} />
          <button type="button" className="tb" disabled={!settings.paneColorLeft} onClick={() => set({ paneColorLeft: "" })}>{tr("settings.paneColors.clear")}</button>
        </div>
        <div className="fld">
          <label htmlFor="pane-color-right">{tr("settings.paneColors.right")}</label>
          <input id="pane-color-right" type="color" value={settings.paneColorRight || themes[settings.theme].panel} onChange={e => set({ paneColorRight: e.target.value })} />
          <button type="button" className="tb" disabled={!settings.paneColorRight} onClick={() => set({ paneColorRight: "" })}>{tr("settings.paneColors.clear")}</button>
        </div>
        <div className="fld">
          <label htmlFor="pane-tint-strength">{tr("settings.paneTintStrength")}</label>
          <input id="pane-tint-strength" type="range" min={80} max={100} step={5} value={settings.paneTintStrength ?? 100} disabled={!settings.paneColorLeft && !settings.paneColorRight} aria-valuetext={`${settings.paneTintStrength ?? 100}%`} onChange={e => set({ paneTintStrength: Number(e.target.value) })} />
          <output htmlFor="pane-tint-strength">{settings.paneTintStrength ?? 100}%</output>
        </div>
        <div className="fld">
          <span id="set-theme">{tr("settings.theme")}</span>
          <Segmented
            label={tr("settings.theme")}
            value={settings.theme}
            options={themeNames.map((name) => ({ value: name, text: tr(`settings.theme.${name}` as TranslationKey) }))}
            onChange={(theme) => {
              if (settings.accentAutomatic) setHex(defaultAccent(theme));
              set({ theme });
            }}
          />
        </div>
        <div className="fld">
          <span>{tr("settings.accent")}</span>
          <div className="accent">
            <button className="tb" aria-pressed={!!settings.accentAutomatic} onClick={() => { setHex(defaultAccent(settings.theme)); set({ accentAutomatic: true }); }}>{tr("settings.accent.default")}</button>
            <div role="group" aria-label={tr("ui.settings.accentPresets")} className="swatches">
              {accentPresets.map((preset) => (
                <button
                  key={preset}
                  type="button"
                  className={currentAccent.toLowerCase() === preset ? "swatch on" : "swatch"}
                  style={{ background: preset } as CSSProperties}
                  aria-label={preset}
                  aria-pressed={currentAccent.toLowerCase() === preset}
                  onClick={() => {
                    setHex(preset);
                    set({ accent: preset, accentAutomatic: false });
                  }}
                />
              ))}
            </div>
            <input
              className="hex"
              aria-label={tr("settings.accent.custom")}
              value={hex}
              maxLength={7}
              spellCheck={false}
              aria-invalid={warnings.includes("invalid")}
              onChange={(event) => onHex(event.target.value)}
            />
          </div>
        </div>
        <div className="warning-slot">{warnings.length > 0 ? (
          <ul className="field-error warnings" role="alert">
            {warnings.map((warning) => (
              <li key={warning}>{tr(accentWarningKeys[warning])}</li>
            ))}
          </ul>
        ) : null}</div>
        <div className="fld">
          <span>{tr("settings.density")}</span>
          <Segmented
            label={tr("settings.density")}
            value={settings.density}
            options={(["compact", "normal", "comfortable"] as const).map((value) => ({
              value,
              text: tr(`settings.density.${value}` as TranslationKey),
            }))}
            onChange={(density) => set({ density })}
          />
        </div>
        <div className="fld">
          <label htmlFor="set-font">{tr("settings.fontSize")}</label>
          <div className="range">
            <input
              id="set-font"
              type="range"
              min={FONT_SIZE_MIN}
              max={FONT_SIZE_MAX}
              step={1}
              value={settings.fontSize}
              onChange={(event) => set({ fontSize: Number(event.target.value) })}
            />
            <output htmlFor="set-font">{tr("settings.fontSize.value", { size: settings.fontSize })}</output>
          </div>
        </div>
        <div className="fld">
          <span>{tr("settings.language")}</span>
          <Segmented
            label={tr("settings.language")}
            value={settings.language}
            options={(["auto", "de", "en"] as const).map((value) => ({
              value,
              text: tr(`settings.language.${value}` as TranslationKey),
            }))}
            onChange={(language) => set({ language })}
          />
        </div>
        <TerminalColorSettings set={set} />
        </>}
        {tab === "system" && <>
        <div className="fld">
          <label htmlFor="verify-policy">{tr("settings.verifyPolicy")}</label>
          <select id="verify-policy" value={settings.verifyPolicy} aria-describedby="verify-help" onChange={e => set({ verifyPolicy: e.target.value as Settings["verifyPolicy"] })}>
            {(["auto", "always", "never"] as const).map(value => <option key={value} value={value}>{tr(`settings.verifyPolicy.${value}`)}</option>)}
          </select>
          <p id="verify-help" className="field-help">{tr("settings.verifyPolicy.help")}</p>
        </div>
        <div className="fld">
          <label htmlFor="terminal-shell">{tr("settings.terminalShell")}</label>
          <select id="terminal-shell" value={settings.terminalShell ?? "login"} aria-describedby="terminal-shell-help" onChange={e => set({ terminalShell: e.target.value as Settings["terminalShell"] })}>
            {(["auto", "bash", "login"] as const).map(value => <option key={value} value={value}>{tr(`settings.terminalShell.${value}`)}</option>)}
          </select>
          <p id="terminal-shell-help" className="field-help">{tr("settings.terminalShell.help")}</p>
        </div>
          <SystemSettings />
        </>}
        {tab === "info" && <><AboutSettings /><UpdateSettings /><TroubleshootSettings /></>}
        {tab === "keyboard" && <KeybindingsSettings />}
      </div>
    </>
  );
}

export function SettingsDialog() {
  const controller = useController();
  const tr = useT();
  const startTab = useAppState(state => state.dialog?.kind === "settings" ? state.dialog.tab : undefined);
  return <Modal wide title={tr("settings.title")} onClose={() => controller.closeDialog()} footer={
    <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeDialog()}>{tr("updates.done")}</button>
  }><SettingsPanel startTab={startTab} /></Modal>;
}
