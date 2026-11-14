import { useEffect, useRef, type KeyboardEvent } from "react";
import { useT } from "../../app/context";

interface FindBarProps {
  query: string;
  onQuery(text: string): void;
  matchCase: boolean;
  onMatchCase(on: boolean): void;
  /** Nur in der Hex-Ansicht: Suchbegriff als Bytefolge lesen. */
  bytes?: { on: boolean; onChange(on: boolean): void };
  onNext(): void;
  onPrev(): void;
  onClose(): void;
  searching: boolean;
  message: string | null;
  canContinue: boolean;
}

/** Suchleiste (Strg+F). Enter = weiter, Umschalt+Enter = zurück; Esc regelt die Ansicht darüber. */
export function FindBar(props: FindBarProps) {
  const tr = useT();
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.key !== "Enter") return;
    event.preventDefault();
    if (event.shiftKey) props.onPrev();
    else props.onNext();
  }
  return (
    <div className="lv-find" role="search" aria-label={tr("lister.find")}>
      <input
        ref={input}
        type="search"
        value={props.query}
        placeholder={tr("lister.findPlaceholder")}
        aria-label={tr("lister.find")}
        spellCheck={false}
        onChange={(event) => props.onQuery(event.target.value)}
        onKeyDown={onKeyDown}
      />
      {props.bytes ? (
        <label className="check">
          <input type="checkbox" checked={props.bytes.on} onChange={(event) => props.bytes?.onChange(event.target.checked)} />
          {tr("lister.findBytes")}
        </label>
      ) : null}
      <label className="check">
        <input type="checkbox" checked={props.matchCase} onChange={(event) => props.onMatchCase(event.target.checked)} />
        {tr("lister.matchCase")}
      </label>
      <button type="button" className="tb small" onClick={props.onPrev}>
        {tr("lister.findPrev")}
      </button>
      <button type="button" className="tb small" onClick={props.onNext}>
        {props.canContinue ? tr("lister.findNext") + " …" : tr("lister.findNext")}
      </button>
      <button type="button" className="tb small" aria-label={tr("action.close")} onClick={props.onClose}>
        ×
      </button>
      <span className="lv-msg" role="status">
        {props.searching ? tr("lister.searching") : props.message}
      </span>
    </div>
  );
}

interface GotoBarProps {
  placeholder: string;
  error: string | null;
  onSubmit(value: string): void;
  onClose(): void;
}

/** Sprungleiste (Strg+G): Enter springt, Esc schließt (über die Ansicht). */
export function GotoBar({ placeholder, error, onSubmit, onClose }: GotoBarProps) {
  const tr = useT();
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  return (
    <form
      className="lv-find"
      aria-label={tr("lister.goto")}
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit(input.current?.value ?? "");
      }}
    >
      <input ref={input} type="text" aria-label={tr("lister.goto")} placeholder={placeholder} spellCheck={false} />
      <button type="submit" className="tb small">
        {tr("lister.go")}
      </button>
      <button type="button" className="tb small" aria-label={tr("action.close")} onClick={onClose}>
        ×
      </button>
      <span className="lv-msg field-error" role="status">
        {error}
      </span>
    </form>
  );
}
