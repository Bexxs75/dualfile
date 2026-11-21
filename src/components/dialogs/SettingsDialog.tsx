import { useEffect, useState, useRef } from "react";
import { useAppState, useController, useLanguage, useRuntime, useT } from "../../app/context";
import type { TranslationKey } from "../../i18n";
import type { Settings } from "../../ipc/types";
import {
  accentPresets,
  accentWarning,
  accentWarningKeys,
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
import { AboutSettings, SystemSettings, TroubleshootSettings, UpdateSettings, useSystem } from "./SystemDialogs";
import { KeybindingsSettings } from "./KeybindingsSettings";
import { actionCatalog, catalog, sections, sectionIcons, resolveSection, normalizeSearch, settingChanged, settingVisible, settingDefault, resetSetting, settingSearch, type SettingEntry, type Section } from "../../settings/catalog";
import { REBINDABLE, REBINDABLE_COMMANDS, effectiveBindings } from "../../state/keymap";
import { SettingsPreview } from "../../settings/SettingsPreview";
import { Highlight } from "../../settings/Highlight";
import { Icon } from "../Icon";
import { Modal } from "../Modal";

function Segmented<V extends string>({
  labelledBy,
  value,
  options,
  onChange,
  query = "",
}: {
  labelledBy: string;
  query?: string;
  value: V;
  options: { value: V; text: string }[];
  onChange: (value: V) => void;
}) {
  return (
    <div className="seg" role="radiogroup" aria-labelledby={labelledBy}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={option.value === value ? "on" : undefined}
          onClick={() => onChange(option.value)}
        >
          <Highlight text={option.text} query={query} />
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

function TerminalColorSettings({ set, query }: { query: string; set: (patch: Partial<Settings>) => void }) {
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
                <b><Highlight text={tr(`settings.terminal.mode.${value}` as TranslationKey)} query={query} /></b>
              </button>
            );
          })}
        </div>
      </div>
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


function SettingControl({ entry, query }: { entry: SettingEntry; query: string }) {
  const settings = useAppState(s => s.settings), controller = useController(), tr = useT();
  const { system } = useRuntime();
  const language = useLanguage();
  const { state: systemState } = useSystem();
  const id = entry.id, label = tr(entry.label), value = settings[id] ?? settingDefault(id);
  const set = (patch: Partial<Settings>) => controller.updateSettings(s => ({ ...s, ...patch }));
  const change = (next: unknown) => { set({ [id]: next }); if (id === "trayEnabled") void system.setTray(Boolean(next), language); };
  const currentAccent = settings.accentAutomatic ? defaultAccent(settings.theme) : settings.accent;
  const [hex, setHex] = useState(currentAccent);
  useEffect(() => setHex(currentAccent), [currentAccent]);
  const warnings = accentWarning(hex, settings.theme);
  const optionText = (option: string) => tr(`${entry.optionPrefix ?? entry.label}.${option}` as TranslationKey);
  if (entry.kind === "switch" || entry.kind === "check") return <>
    <input id={`setting-${id}`} aria-describedby={entry.help ? `help-${id}` : undefined} type="checkbox" role={entry.kind === "switch" ? "switch" : undefined} aria-checked={Boolean(value)} checked={Boolean(value)} onChange={e => change(e.target.checked)} />
    {id === "trayEnabled" && systemState.trayUnavailable && <p className="field-error" role="status">{tr("system.trayUnavailable")}</p>}
  </>;
  if (entry.kind === "range") {
    const disabled = id === "paneTintStrength" && !settings.paneColorLeft && !settings.paneColorRight;
    return <><div className="range"><input id={`setting-${id}`} type="range" min={entry.min} max={entry.max} step={entry.step} value={Number(value)} disabled={disabled} aria-valuetext={`${value}${id === "paneTintStrength" ? "%" : " px"}`} onChange={e => change(Number(e.target.value))} /><output htmlFor={`setting-${id}`}>{String(value)}{id === "paneTintStrength" ? "%" : " px"}</output></div>{disabled && <p className="field-help">{tr("settings.choosePaneColor")}</p>}</>;
  }
  if (entry.kind === "segment") return id === "verifyPolicy" ? <div className="settings-radios" role="radiogroup" aria-labelledby={`label-${id}`} aria-describedby="verify-help">{entry.options!.map(option => <label key={option}><input type="radio" name={id} checked={value === option} onChange={() => change(option)} /><Highlight text={optionText(option)} query={query} /></label>)}</div> : <Segmented query={query} labelledBy={`label-${id}`} value={String(value)} options={entry.options!.map(option => ({ value: option, text: optionText(option) }))} onChange={change} />;
  if (entry.kind === "select") return <select id={`setting-${id}`} aria-describedby={entry.help ? `help-${id}` : undefined} value={String(value)} onChange={e => change(e.target.value)}>{entry.options!.map(option => <option key={option} value={option}>{optionText(option)}</option>)}</select>;
  switch (id) {
    case "theme": return <div className="settings-themes" role="radiogroup" aria-labelledby={`label-${id}`}>{themeNames.map(theme => <button key={theme} type="button" role="radio" aria-checked={settings.theme === theme} onClick={() => set({ theme })}><i aria-hidden="true" style={{ background: themes[theme].panel, borderColor: themes[theme].line, color: themes[theme].fg }}><span style={{ background: themes[theme].accent }} /></i><Highlight text={tr(`settings.theme.${theme}`)} query={query} /></button>)}</div>;
    case "accent": return <><button type="button" className="tb" aria-pressed={!!settings.accentAutomatic} onClick={() => set({ accentAutomatic: true })}>{tr("settings.accent.default")}</button><div className="accent"><div role="group" aria-label={tr("ui.settings.accentPresets")} className="swatches">{accentPresets.map(preset => <button key={preset} type="button" className="swatch" style={{ background: preset }} aria-label={preset} aria-pressed={currentAccent.toLowerCase() === preset} onClick={() => set({ accent: preset, accentAutomatic: false })} />)}</div><input className="hex" aria-label={tr("settings.accent.custom")} value={hex} maxLength={7} spellCheck={false} aria-invalid={warnings.includes("invalid")} onChange={e => { const raw = e.target.value.trim(); const next = raw && !raw.startsWith("#") ? `#${raw}` : raw; setHex(next); if (isValidHex(next)) set({ accent: next.toLowerCase(), accentAutomatic: false }); }} /></div><div className="warning-slot">{warnings.length > 0 && <ul className="field-error warnings" role="alert">{warnings.map(warning => <li key={warning}>{tr(accentWarningKeys[warning])}</li>)}</ul>}</div></>;
    case "paneColorLeft": case "paneColorRight": return <div className="settings-color"><span className={value ? "color-box" : "color-box none"} style={value ? { background: String(value) } : undefined}><input id={`setting-${id}`} type="color" value={String(value || "#000000")} onChange={e => change(e.target.value)} /></span>{!value && <span>{tr("settings.none")}</span>}<button type="button" className="tb" disabled={!value} onClick={() => change("")}>{tr("settings.paneColors.clear")}</button></div>;
    case "sidebarCollapsed": return <div className="settings-checks">{(["places", "remote", "devices"] as const).map(area => <label key={area}><input type="checkbox" checked={!(settings.sidebarCollapsed ?? []).includes(area)} onChange={e => change(e.target.checked ? (settings.sidebarCollapsed ?? []).filter(v => v !== area) : [...(settings.sidebarCollapsed ?? []), area])} /><Highlight text={tr(area === "devices" ? "places.drives" : `sidebar.${area}`)} query={query} /></label>)}</div>;
    case "sidebarHidden": return <><button type="button" className="tb" onClick={() => change([])}>{label}</button><p className="field-help" role="status">{tr(settings.sidebarHidden?.length ? "settings.hiddenCount" : "settings.nothingHidden", { n: settings.sidebarHidden?.length ?? 0 })}</p></>;
    case "terminalColors": return <TerminalColorSettings set={set} query={query} />;
    case "terminalBg": case "terminalFg": return <><ColorField id={`setting-${id}`} label={label} hex={String(value)} onValid={change} />{id === "terminalFg" && <button type="button" className="link" onClick={() => set({ terminalBg: settings.terminalFg ?? DEFAULT_TERMINAL_FG, terminalFg: settings.terminalBg ?? DEFAULT_TERMINAL_BG })}>{tr("settings.terminal.swap")}</button>}</>;
    default: return null;
  }
}

export function SettingsPanel({ startTab, onClose }: { startTab?: import("../../ipc/types").SettingsTab; onClose?: () => void }) {
  const controller = useController(), tr = useT(), language = useLanguage();
  const { system, state: systemState } = useSystem();
  const settings = useAppState(s => s.settings);
  const [tab, setTab] = useState<Section>(() => resolveSection(startTab));
  const [query, setQuery] = useState(""), [modified, setModified] = useState(false), [confirmReset, setConfirmReset] = useState(false);
  const [resetError, setResetError] = useState("");
  const searchRef = useRef<HTMLInputElement>(null), contentRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    setTab(resolveSection(startTab));
    setQuery("");
    setModified(false);
    contentRef.current?.scrollTo?.(0, 0);
  }, [startTab]);
  useEffect(() => { if (!confirmReset) return; const timer = setTimeout(() => setConfirmReset(false), 3500); return () => clearTimeout(timer); }, [confirmReset]);
  const changed = catalog.filter(entry => settingChanged(settings, entry.id));
  const bindings = effectiveBindings(settings.keybindings);
  const changedKeys = REBINDABLE_COMMANDS.filter(command => bindings[command] !== REBINDABLE[command]);
  const autostartChanged = !!systemState.status && systemState.status.autostart !== "off";
  const count = changed.length + changedKeys.length + Number(autostartChanged);
  const needle = normalizeSearch(query.trim()), filtering = !!needle || modified;
  const matches = (text: string) => normalizeSearch(text).includes(needle);
  const entries = catalog.filter(entry => (entry.id !== "checkUpdatesOnStart" || systemState.status?.packaging !== "flatpak") && settingVisible(settings, entry.id) && (!modified || settingChanged(settings, entry.id)) && matches(settingSearch(entry, tr)));
  const commands = REBINDABLE_COMMANDS.filter(command => (!modified || changedKeys.includes(command)) && matches(`${tr("settings.section.tastatur")} ${tr("settings.keys.help")} ${tr(`keys.cmd.${command}` as TranslationKey)} ${bindings[command]}`));
  const actions = actionCatalog.filter(action =>
    (action.id !== "startMenu" || systemState.status?.packaging !== "flatpak") &&
    (!modified || action.id === "autostart" && autostartChanged) &&
    matches([tr(`settings.section.${action.section}`), action.group ? tr(`settings.group.${action.group}` as TranslationKey) : "", ...action.text.map(key => tr(key))].join(" "))
  );
  const hits = (section: Section) => entries.filter(e => e.section === section).length + actions.filter(a => a.section === section).length + (section === "tastatur" ? commands.length : 0);
  const open = (section: Section) => { setTab(section); setQuery(""); setModified(false); contentRef.current?.scrollTo?.(0, 0); };
  const resetAutostart = async () => { try { await system.setAutostart("off"); } catch (e) { setResetError(String(e)); } };
  const resetAll = () => {
    if (!confirmReset) { setConfirmReset(true); return; }
    setConfirmReset(false); setResetError("");
    controller.updateSettings(s => ({ ...catalog.reduce((next, entry) => resetSetting(next, entry.id), s), keybindings: {} }));
    if (settings.trayEnabled) void system.setTray(false, language);
    if (autostartChanged) void resetAutostart();
  };
  const reset = (entry: SettingEntry) => {
    controller.updateSettings(s => resetSetting(s, entry.id));
    if (entry.id === "trayEnabled") void system.setTray(false, language);
  };
  const sectionChanged = (section: Section) => changed.some(e => e.section === section) || section === "tastatur" && changedKeys.length > 0 || section === "allgemein" && autostartChanged;
  const dot = <span className="modified-dot" title={tr("settings.changed")}><span aria-hidden="true">●</span><span className="sr-only"> {tr("settings.changed")}</span></span>;
  const row = (entry: SettingEntry) => {
    const labelledControl = ["switch", "check", "range", "select"].includes(entry.kind) || ["paneColorLeft", "paneColorRight", "terminalBg", "terminalFg"].includes(entry.id);
    const groupedControl = !labelledControl && entry.kind !== "segment" && entry.id !== "theme";
    const label = <><Highlight text={tr(entry.label)} query={query} />{entry.rare && <small className="rare" aria-hidden="true">{tr("settings.rare")}</small>}</>;
    return <div className="setting-row" key={entry.id} data-setting={entry.id} role={groupedControl ? "group" : undefined} aria-labelledby={groupedControl ? `label-${entry.id}` : undefined}>
      {labelledControl ? <label htmlFor={`setting-${entry.id}`}>{label}</label> : <span className="setting-label" id={`label-${entry.id}`}>{label}</span>}
      <div className="setting-control"><SettingControl entry={entry} query={query} />{entry.help && <p id={entry.id === "verifyPolicy" ? "verify-help" : `help-${entry.id}`} className="field-help"><Highlight text={tr(entry.help)} query={query} /></p>}</div>
      <div className="setting-reset">{settingChanged(settings, entry.id) && <>{dot}<button className="link" type="button" aria-label={`${tr("settings.reset")}: ${tr(entry.label)}`} onClick={() => reset(entry)}>{tr("settings.reset")}</button></>}</div>
    </div>;
  };
  const renderAction = (id: string) => <div key={id} className="settings-action">{id === "autostart" || id === "startMenu" ? <SystemSettings field={id} query={query} onReset={id === "autostart" && autostartChanged ? () => void resetAutostart() : undefined} /> : id === "about" ? <AboutSettings query={query} /> : id === "updates" ? <UpdateSettings query={query} /> : <TroubleshootSettings query={query} />}</div>;
  const renderSection = (section: Section) => {
    const selected = entries.filter(e => e.section === section);
    return <section key={section} className="settings-page">
      {filtering ? <h2><button className="link" onClick={() => open(section)}><Highlight text={tr(`settings.section.${section}`)} query={query} /></button></h2> : <><h2>{tr(`settings.section.${section}`)}</h2><p className="muted">{tr(`settings.sub.${section}`)}</p></>}
      {!filtering && (section === "darstellung" || section === "dateiliste") && <SettingsPreview />}
      {[...new Set(selected.map(entry => entry.group))].map(group => <section className="settings-group" key={group}><h3><Highlight text={tr(`settings.group.${group}` as TranslationKey)} query={query} /></h3>{section === "allgemein" && group === "start" && actions.filter(a => a.id === "autostart").map(a => renderAction(a.id))}{selected.filter(entry => entry.group === group && (filtering || entry.kind !== "check")).map(row)}{!filtering && selected.some(entry => entry.group === group && entry.kind === "check") && <fieldset className="settings-extra-columns"><legend>{tr("settings.extraColumns")}</legend><div>{selected.filter(entry => entry.group === group && entry.kind === "check").map(row)}</div></fieldset>}{section === "allgemein" && group === "start" && actions.filter(a => a.id === "startMenu").map(a => renderAction(a.id))}</section>)}
      {actions.filter(a => a.section === section && (section !== "allgemein" || !selected.some(e => e.group === "start"))).map(a => renderAction(a.id))}
      {section === "tastatur" && <KeybindingsSettings commands={commands} query={query} />}
    </section>;
  };
  return <div className="settings-shell" onKeyDown={e => { if (e.ctrlKey && e.key.toLowerCase() === "f") { e.preventDefault(); e.stopPropagation(); searchRef.current?.focus(); } }}>
    <div className="settings-layout"><aside className="settings-side">
      <input ref={searchRef} className="settings-search" type="search" aria-label={tr("settings.search")} placeholder={tr("settings.search")} value={query} onChange={e => setQuery(e.target.value)} onKeyDown={e => { if (e.key === "Escape" && query) { e.preventDefault(); e.stopPropagation(); setQuery(""); } }} />
      <div className="settings-filters"><button className="tb" aria-pressed={!modified} onClick={() => setModified(false)}>{tr("settings.all")}</button><button className="tb" aria-pressed={modified} onClick={() => setModified(true)}>{tr("settings.modified", { n: count })}</button></div>
      <div className="settings-nav" role="tablist" aria-label={tr("settings.tabs")} aria-orientation="vertical">{sections.map((section, index) => <button key={section} type="button" role="tab" aria-label={[tr(`settings.section.${section}`), sectionChanged(section) ? tr("settings.changed") : "", filtering ? hits(section) : ""].filter(value => value !== "").join(" ")} id={`settings-tab-${section}`} aria-controls="settings-panel" aria-selected={!filtering && tab === section} tabIndex={tab === section ? 0 : -1} className={filtering && !hits(section) ? "dim" : undefined} onClick={() => open(section)} onKeyDown={e => {
        const next = e.key === "Home" ? 0 : e.key === "End" ? sections.length - 1 : e.key === "ArrowDown" || e.key === "ArrowRight" ? (index + 1) % sections.length : e.key === "ArrowUp" || e.key === "ArrowLeft" ? (index + sections.length - 1) % sections.length : -1;
        if (next >= 0) { e.preventDefault(); open(sections[next]!); document.getElementById(`settings-tab-${sections[next]}`)?.focus(); }
      }}><Icon name={sectionIcons[section]} /><span>{tr(`settings.section.${section}`)}</span>{sectionChanged(section) && dot}{filtering && <span className="settings-hit-count"> {hits(section)}</span>}</button>)}</div>
    </aside><div ref={contentRef} className="settings-content" id="settings-panel" role="tabpanel" tabIndex={0} aria-label={filtering ? tr("settings.results") : tr(`settings.section.${tab}`)}>
      {filtering ? sections.some(section => hits(section) > 0) ? sections.filter(section => hits(section) > 0).map(renderSection) : <p role="status">{tr(needle ? "settings.emptySearch" : "settings.emptyModified")}</p> : renderSection(tab)}
    </div></div>
    <footer className="settings-footer"><span>{tr("settings.immediate", { n: count })}</span><button className="tb" onClick={resetAll}>{tr(confirmReset ? "settings.confirmReset" : "settings.resetAll")}</button>{onClose && <button className="tb primary" data-autofocus onClick={onClose}>{tr("updates.done")}</button>}{resetError && <p role="alert">{resetError}</p>}</footer>
  </div>;
}

export function SettingsDialog() {
  const controller = useController(), tr = useT();
  const startTab = useAppState(state => state.dialog?.kind === "settings" ? state.dialog.tab : undefined);
  return <Modal className="settings-dialog" title={tr("settings.title")} onClose={() => controller.closeDialog()}><SettingsPanel startTab={startTab} onClose={() => controller.closeDialog()} /></Modal>;
}
