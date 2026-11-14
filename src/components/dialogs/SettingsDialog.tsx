import { useState, type CSSProperties } from "react";
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
  defaultAccent,
} from "../../themes/tokens";
import { SystemSettings } from "./SystemDialogs";
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

/** Aussehen und Sprache. Änderungen gelten sofort (ohne Neustart) und werden entprellt gespeichert. */
export function SettingsDialog() {
  const controller = useController();
  const tr = useT();
  const settings = useAppState((state) => state.settings);
  const [hex, setHex] = useState(settings.accentAutomatic ? defaultAccent(settings.theme) : settings.accent);
  const set = (patch: Partial<Settings>) => controller.updateSettings((s) => ({ ...s, ...patch }));
  const currentAccent = settings.accentAutomatic ? defaultAccent(settings.theme) : settings.accent;
  const warnings = accentWarning(hex, settings.theme);

  function onHex(value: string): void {
    setHex(value);
    if (isValidHex(value)) set({ accent: value.toLowerCase(), accentAutomatic: false });
  }

  return (
    <Modal
      wide
      title={tr("settings.title")}
      onClose={() => controller.closeDialog()}
      footer={
        <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeDialog()}>
          {tr("updates.done")}
        </button>
      }
    >
      <div className="settings">
        <label className="fld"><span>{tr("settings.showParentEntry")}</span><input type="checkbox" checked={settings.showParentEntry !== false} onChange={event => set({ showParentEntry: event.target.checked })} /></label>
        <label className="fld"><span>{tr("settings.extensionColumn")}</span><input type="checkbox" checked={settings.extensionColumn === true} onChange={event => set({ extensionColumn: event.target.checked })} /></label>
        <div className="fld">
          <label htmlFor="verify-policy">{tr("settings.verifyPolicy")}</label>
          <select id="verify-policy" value={settings.verifyPolicy} aria-describedby="verify-help" onChange={e => set({ verifyPolicy: e.target.value as Settings["verifyPolicy"] })}>
            {(["auto", "always", "never"] as const).map(value => <option key={value} value={value}>{tr(`settings.verifyPolicy.${value}`)}</option>)}
          </select>
          <p id="verify-help" className="field-help">{tr("settings.verifyPolicy.help")}</p>
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
        <SystemSettings />
      </div>
    </Modal>
  );
}
