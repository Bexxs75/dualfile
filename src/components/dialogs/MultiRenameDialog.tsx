import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useAppState, useController, useT } from "../../app/context";
import type { Dialog } from "../../app/state";
import type { MediaTags } from "../../ipc/types";
import {
  PLACEHOLDERS,
  defaultRules,
  emptyTags,
  needsRegex,
  planRename,
  regexTargets,
  type CaseMode,
  type MultiRenameInput,
  type MultiRenameRules,
  type RegexOutcome,
  type RenamePreset,
} from "../../state/multirename";
import { baseName, joinPath } from "../../state/paths";
import { Modal } from "../Modal";

/** Wartezeit, bis eine Regex-Vorschau beim Kern angefragt wird (Tippen löst nicht jeden Buchstaben aus). */
export const REGEX_DEBOUNCE_MS = 200;

const CASE_MODES: readonly CaseMode[] = ["keep", "upper", "lower", "words", "first"];

type MultiRenameState = Extract<Dialog, { kind: "multiRename" }>;

/** Zahlenfeld → Zahl; leer oder kein Zahlenwert ergibt NaN (und damit einen Fehler am Zähler). */
const toNumber = (text: string): number => (text.trim() === "" ? Number.NaN : Number(text));

/**
 * Mehrfach-Umbenennen (Strg+M): Regeln links oben, Vorschau darunter. Umbenannt wird erst auf
 * ausdrücklichen Knopfdruck; „Abbrechen“ ist der Standardknopf, Enter in einem Feld löst nichts aus.
 */
export function MultiRenameDialog({ dialog }: { dialog: MultiRenameState }) {
  const controller = useController();
  const tr = useT();
  const presets = useAppState((state) => state.renamePresets);

  const [nameMask, setNameMask] = useState(defaultRules.nameMask);
  const [extMask, setExtMask] = useState(defaultRules.extMask);
  const [start, setStart] = useState(String(defaultRules.counterStart));
  const [step, setStep] = useState(String(defaultRules.counterStep));
  const [digits, setDigits] = useState(String(defaultRules.counterDigits));
  const [search, setSearch] = useState("");
  const [replace, setReplace] = useState("");
  const [searchMode, setSearchMode] = useState<MultiRenameRules["searchMode"]>("text");
  const [ignoreCase, setIgnoreCase] = useState(false);
  const [all, setAll] = useState(true);
  const [caseMode, setCaseMode] = useState<CaseMode>("keep");
  const [busy, setBusy] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const [menuOpen, setMenuOpen] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const extRef = useRef<HTMLInputElement>(null);
  const lastMask = useRef<"name" | "ext">("name");
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);

  const [presetName, setPresetName] = useState("");
  const [presetChoice, setPresetChoice] = useState("");

  const ids = { name: useId(), ext: useId(), start: useId(), step: useId(), digits: useId(), search: useId(), replace: useId(), mode: useId(), caseId: useId(), preset: useId(), presetName: useId(), hint: useId() };

  const rules: MultiRenameRules = useMemo(
    () => ({
      nameMask,
      extMask,
      counterStart: toNumber(start),
      counterStep: toNumber(step),
      counterDigits: toNumber(digits),
      search,
      replace,
      searchMode,
      ignoreCase,
      all,
      caseMode,
    }),
    [nameMask, extMask, start, step, digits, search, replace, searchMode, ignoreCase, all, caseMode],
  );
  // EXIF-/ID3-Tags einmal beim Öffnen laden; bis dahin leere Tags, danach wird die Vorschau neu berechnet.
  const [tags, setTags] = useState<readonly MediaTags[] | null>(null);
  useEffect(() => {
    let current = true;
    controller.api
      .mediaTags(dialog.sources.map((source) => joinPath(dialog.dir, source.name)))
      .then((loaded) => {
        if (current) setTags(loaded);
      })
      // Ein Ladefehler zählt wie „keine Tags vorhanden" (leere Werte, nicht mehr `pending`),
      // damit Masken mit Tag-Platzhaltern nicht dauerhaft gesperrt bleiben.
      .catch(() => {
        if (current) setTags(dialog.sources.map(() => emptyTags));
      });
    return () => {
      current = false;
    };
  }, [controller, dialog.dir, dialog.sources]);
  const sources = useMemo(
    () => (tags === null || tags.length !== dialog.sources.length ? dialog.sources : dialog.sources.map((source, i) => ({ ...source, tags: tags[i] ?? source.tags }))),
    [dialog.sources, tags],
  );
  const input: MultiRenameInput = useMemo(
    () => ({ sources, rules, parentName: baseName(dialog.dir) === "/" ? "" : baseName(dialog.dir), existingNames: dialog.existing, tagsPending: tags === null }),
    [dialog, sources, rules, tags],
  );

  // Regex: nur der Kern wertet aus. Anfrage entprellt; Antworten zu älteren Eingaben werden verworfen.
  const regexKey = useMemo(() => {
    if (!needsRegex(rules)) return null;
    const targets = regexTargets(input);
    if (targets.length === 0) return null;
    return JSON.stringify([targets, rules.search, rules.replace, rules.ignoreCase, rules.all]);
  }, [input, rules]);
  const [answer, setAnswer] = useState<{ key: string; outcome: RegexOutcome } | null>(null);
  useEffect(() => {
    if (regexKey === null) return;
    let current = true;
    const [targets, pattern, replacement, ignore, every] = JSON.parse(regexKey) as [string[], string, string, boolean, boolean];
    const timer = setTimeout(() => {
      controller.api
        .regexReplace(targets, pattern, replacement, ignore, every)
        .then((results) => {
          if (current) setAnswer({ key: regexKey, outcome: { kind: "results", results } });
        })
        .catch((error: unknown) => {
          if (current) setAnswer({ key: regexKey, outcome: { kind: "failed", message: error instanceof Error ? error.message : String(error) } });
        });
    }, REGEX_DEBOUNCE_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [regexKey, controller]);
  const outcome = regexKey !== null && answer?.key === regexKey ? answer.outcome : undefined;
  const pending = regexKey !== null && outcome === undefined;

  const plan = useMemo(() => planRename(input, outcome), [input, outcome]);
  const canRename = plan.canApply && !pending && !busy;

  function insertPlaceholder(text: string): void {
    const field = lastMask.current === "ext" ? extRef.current : nameRef.current;
    const setter = lastMask.current === "ext" ? setExtMask : setNameMask;
    const current = field?.value ?? "";
    const from = field?.selectionStart ?? current.length;
    const to = field?.selectionEnd ?? from;
    setter(current.slice(0, from) + text + current.slice(to));
    setMenuOpen(false);
    const caret = from + text.length;
    requestAnimationFrame(() => {
      field?.focus();
      field?.setSelectionRange(caret, caret);
    });
  }

  function onMenuKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const buttons = [...(menuRef.current?.querySelectorAll<HTMLElement>("button") ?? [])];
    const index = buttons.indexOf(document.activeElement as HTMLElement);
    if (event.key === "ArrowDown") buttons[(index + 1) % buttons.length]?.focus();
    else if (event.key === "ArrowUp") buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
    else if (event.key === "Home") buttons[0]?.focus();
    else if (event.key === "End") buttons[buttons.length - 1]?.focus();
    else if (event.key === "Escape") {
      setMenuOpen(false);
      menuButtonRef.current?.focus();
    } else return;
    event.preventDefault();
    event.stopPropagation();
  }

  useEffect(() => {
    if (menuOpen) menuRef.current?.querySelector<HTMLElement>("button")?.focus();
  }, [menuOpen]);

  function loadPreset(preset: RenamePreset): void {
    const r = preset.rules;
    setNameMask(r.nameMask);
    setExtMask(r.extMask);
    setStart(String(r.counterStart));
    setStep(String(r.counterStep));
    setDigits(String(r.counterDigits));
    setSearch(r.search);
    setReplace(r.replace);
    setSearchMode(r.searchMode);
    setIgnoreCase(r.ignoreCase);
    setAll(r.all);
    setCaseMode(r.caseMode);
  }

  async function submit(): Promise<void> {
    if (!canRename) return;
    setBusy(true);
    setSubmitError(null);
    const error = await controller.submitMultiRename(plan.items);
    setBusy(false);
    if (error) setSubmitError(error);
  }

  const chosen = presets.find((preset) => preset.name === presetChoice);
  const noEnter = (event: KeyboardEvent<HTMLInputElement>): void => {
    // Enter in einem Feld bestätigt nie die Umbenennung.
    if (event.key === "Enter") event.preventDefault();
  };
  const text = (value: string, set: (v: string) => void, extra: Record<string, unknown> = {}) => ({
    value,
    spellCheck: false,
    autoComplete: "off",
    onChange: (event: { target: { value: string } }) => set(event.target.value),
    onKeyDown: noEnter,
    ...extra,
  });

  return (
    <Modal
      title={`${tr("multirename.title")} – ${tr("multirename.targets", { count: dialog.sources.length, folder: baseName(dialog.dir) })}`}
      className="mr"
      onClose={() => controller.closeDialog()}
      footer={
        <>
          <button type="button" className="tb primary" onClick={() => controller.closeDialog()}>
            {tr("action.cancel")}
          </button>
          <button type="button" className="tb" disabled={!canRename} onClick={() => void submit()}>
            {tr("multirename.rename")}
          </button>
        </>
      }
    >
      <div className="mr-rules">
        <div className="mr-masks">
          <label className="fld" htmlFor={ids.name}>
            <span>{tr("multirename.nameMask")}</span>
            <input id={ids.name} ref={nameRef} data-autofocus aria-describedby={ids.hint} onFocus={() => (lastMask.current = "name")} {...text(nameMask, setNameMask)} />
          </label>
          <label className="fld" htmlFor={ids.ext}>
            <span>{tr("multirename.extMask")}</span>
            <input id={ids.ext} ref={extRef} onFocus={() => (lastMask.current = "ext")} {...text(extMask, setExtMask)} />
          </label>
          <div className="fld">
            <span />
            <div className="mr-ph-wrap">
              <button
                type="button"
                ref={menuButtonRef}
                className="tb small"
                aria-haspopup="menu"
                aria-expanded={menuOpen}
                onClick={() => setMenuOpen((open) => !open)}
              >
                {tr("multirename.placeholders")} ▾
              </button>
              <span id={ids.hint} className="hint">
                {tr("multirename.maskHint")}
              </span>
              {menuOpen ? (
                <div ref={menuRef} className="mr-ph" role="menu" aria-label={tr("multirename.placeholdersMenu")} onKeyDown={onMenuKeyDown}>
                  {PLACEHOLDERS.map((placeholder) => (
                    <button key={placeholder.insert} type="button" role="menuitem" onClick={() => insertPlaceholder(placeholder.insert)}>
                      <code>{placeholder.insert}</code>
                      <span>{tr(placeholder.label)}</span>
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          </div>
          <fieldset className="mr-counter">
            <legend>{tr("multirename.counter")}</legend>
            <label htmlFor={ids.start}>
              {tr("multirename.counterStart")}
              <input id={ids.start} type="number" step={1} onKeyDown={noEnter} value={start} onChange={(event) => setStart(event.target.value)} />
            </label>
            <label htmlFor={ids.step}>
              {tr("multirename.counterStep")}
              <input id={ids.step} type="number" step={1} onKeyDown={noEnter} value={step} onChange={(event) => setStep(event.target.value)} />
            </label>
            <label htmlFor={ids.digits}>
              {tr("multirename.counterDigits")}
              <input id={ids.digits} type="number" step={1} min={1} max={9} onKeyDown={noEnter} value={digits} onChange={(event) => setDigits(event.target.value)} />
            </label>
          </fieldset>
        </div>

        <div className="mr-search">
          <label className="fld" htmlFor={ids.search}>
            <span>{tr("multirename.search")}</span>
            <input id={ids.search} {...text(search, setSearch)} />
          </label>
          <label className="fld" htmlFor={ids.replace}>
            <span>{tr("multirename.replace")}</span>
            <input id={ids.replace} {...text(replace, setReplace)} />
          </label>
          <div className="fld">
            <label htmlFor={ids.mode}>{tr("multirename.searchMode")}</label>
            <select id={ids.mode} value={searchMode} onChange={(event) => setSearchMode(event.target.value === "regex" ? "regex" : "text")}>
              <option value="text">{tr("multirename.mode.text")}</option>
              <option value="regex">{tr("multirename.mode.regex")}</option>
            </select>
          </div>
          <label className="check">
            <input type="checkbox" checked={ignoreCase} onChange={(event) => setIgnoreCase(event.target.checked)} />
            {tr("multirename.ignoreCase")}
          </label>
          <label className="check">
            <input type="checkbox" checked={all} onChange={(event) => setAll(event.target.checked)} />
            {tr("multirename.all")}
          </label>
          {searchMode === "regex" ? <p className="hint">{tr("multirename.regexHint")}</p> : null}
          <div className="fld">
            <label htmlFor={ids.caseId}>{tr("multirename.case")}</label>
            <select id={ids.caseId} value={caseMode} onChange={(event) => setCaseMode(CASE_MODES.find((mode) => mode === event.target.value) ?? "keep")}>
              {CASE_MODES.map((mode) => (
                <option key={mode} value={mode}>
                  {tr(`multirename.case.${mode}`)}
                </option>
              ))}
            </select>
          </div>
        </div>
      </div>

      <fieldset className="mr-presets">
        <legend>{tr("multirename.presets")}</legend>
        <select aria-label={tr("multirename.presetSelect")} value={presetChoice} onChange={(event) => setPresetChoice(event.target.value)}>
          <option value="">{tr("multirename.presetNone")}</option>
          {presets.map((preset) => (
            <option key={preset.name} value={preset.name}>
              {preset.name}
            </option>
          ))}
        </select>
        <button type="button" className="tb small" disabled={!chosen} onClick={() => chosen && loadPreset(chosen)}>
          {tr("multirename.presetLoad")}
        </button>
        <button
          type="button"
          className="tb small"
          disabled={!chosen}
          onClick={() => {
            if (!chosen) return;
            controller.deletePresetNamed(chosen.name);
            setPresetChoice("");
          }}
        >
          {tr("multirename.presetDelete")}
        </button>
        <input aria-label={tr("multirename.presetName")} placeholder={tr("multirename.presetName")} {...text(presetName, setPresetName, { maxLength: 80 })} />
        <button
          type="button"
          className="tb small"
          disabled={presetName.trim() === "" || rulesInvalid(plan)}
          onClick={() => {
            controller.savePresetNamed(presetName, rules);
            setPresetChoice(presetName.trim());
            setPresetName("");
          }}
        >
          {tr("multirename.presetSave")}
        </button>
        <span className="hint">{tr("multirename.presetSessionOnly")}</span>
      </fieldset>

      <div className="mr-message">
        {plan.rulesError ? (
          <p className="field-error" role="alert">
            {tr(plan.rulesError.key, plan.rulesError.params)}
          </p>
        ) : null}
      </div>
      <div className="mr-preview">
        <p className="mr-summary" role="status" aria-live="polite">
          {pending ? tr("multirename.pending") : tr("multirename.summary", { ok: plan.counts.ok, unchanged: plan.counts.unchanged, errors: plan.counts.error })}
        </p>
        <div className="mr-table-wrap">
          <table className="tbl" aria-label={tr("multirename.preview")}>
            <thead>
              <tr>
                <th scope="col">{tr("multirename.col.old")}</th>
                <th scope="col">{tr("multirename.col.new")}</th>
                <th scope="col">{tr("multirename.col.status")}</th>
              </tr>
            </thead>
            <tbody>
              {plan.rows.map((row) => (
                <tr key={row.oldName} className={row.status} data-status={row.status}>
                  <td className="old">{row.oldName}</td>
                  <td className="new">{row.newName}</td>
                  <td className="st">
                    {row.status === "error" && row.errorKey ? (
                      <span className="field-error">{tr(row.errorKey, row.errorParams)}</span>
                    ) : (
                      tr(`multirename.status.${row.status}`)
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
      {submitError ? (
        <p className="field-error" role="alert">
          {submitError}
        </p>
      ) : null}
    </Modal>
  );
}

const rulesInvalid = (plan: ReturnType<typeof planRename>): boolean => plan.rulesError !== null && plan.rulesError.key === "multirename.error.counterInvalid";
