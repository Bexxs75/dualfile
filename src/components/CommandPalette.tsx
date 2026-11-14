import { useEffect, useId, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { useAppState, useRuntime, useT } from "../app/context";
import { matchCommand, type PaletteCommand } from "../app/commands";
import { Icon } from "./Icon";
import { Modal } from "./Modal";

export function CommandPalette({ commands, recent, onRun, onClose }: { commands: PaletteCommand[]; recent: string[]; onRun: (id: string) => void; onClose: () => void }) {
  const tr = useT();
  const state = useAppState(s => s);
  const { store } = useRuntime();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();
  useEffect(() => {
    const toggle = (event: KeyboardEvent) => {
      if (event.isComposing || !event.ctrlKey || event.shiftKey || event.altKey || event.metaKey || !["k", "p"].includes(event.key.toLowerCase())) return;
      event.preventDefault(); event.stopPropagation();
      if (!event.repeat) onClose();
    };
    document.addEventListener("keydown", toggle, true);
    return () => document.removeEventListener("keydown", toggle, true);
  }, [onClose]);
  const matches = commands.flatMap(command => {
    const hits = matchCommand(command.title, query);
    return hits ? [{ command, hits }] : [];
  });
  const recentMatches = query.trim() ? [] : recent.flatMap(key => matches.filter(m => m.command.id === key));
  const groups = [
    { name: tr("palette.recent"), entries: recentMatches },
    ...(["actions", "bookmarks", "connections", "tabs"] as const).map(group => ({ name: tr(`palette.${group}`), entries: matches.filter(m => m.command.group === group && !recentMatches.includes(m)) })),
  ];
  const entries = groups.flatMap(g => g.entries);
  const current = entries[Math.min(index, entries.length - 1)];
  const optionId = (command: PaletteCommand) => `${id}-${encodeURIComponent(command.id)}`;
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" }); }, [index, query]);
  function run(command: PaletteCommand) {
    if (!command.enabled(store.getState())) return;
    // Unmount first: the old modal restores focus before a command opens a new dialog.
    flushSync(() => { onRun(command.id); onClose(); });
    command.execute();
  }
  return <Modal title={tr("palette.title")} className="command-palette" onClose={onClose} footer={<span id={`${id}-hint`}>{tr("palette.hint")}</span>}>
    <div onKeyDown={event => {
      if (event.nativeEvent.isComposing) return;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault(); event.stopPropagation();
        setIndex(entries.length ? (entries.indexOf(current!) + (event.key === "ArrowDown" ? 1 : -1) + entries.length) % entries.length : 0);
      } else if (event.key === "Enter") {
        event.preventDefault(); event.stopPropagation(); if (current && !event.repeat) run(current.command);
      }
    }}>
      <label className="palette-search"><Icon name="search" /><input data-autofocus role="combobox" aria-label={tr("palette.title")} aria-expanded="true" aria-autocomplete="list" aria-controls={`${id}-list`} aria-describedby={`${id}-hint`} aria-activedescendant={current ? optionId(current.command) : undefined} value={query} placeholder={tr("header.command")} onChange={e => { setQuery(e.target.value); setIndex(0); }} /></label>
      <div ref={list} id={`${id}-list`} className="palette-list" role="listbox" aria-label={tr("palette.title")}>
        {groups.filter(g => g.entries.length).map(group => <div role="group" aria-label={group.name} key={group.name}>
          <div className="palette-heading" aria-hidden="true">{group.name}</div>
          {group.entries.map(({ command, hits }) => <div key={command.id} id={optionId(command)} role="option" aria-selected={current?.command.id === command.id} aria-disabled={!command.enabled(state)} className="palette-option" onMouseDown={e => e.preventDefault()} onMouseMove={() => setIndex(entries.findIndex(m => m.command.id === command.id))} onClick={() => run(command)}>
            <Icon name={command.icon} /><span>{Array.from(command.title).map((char, i) => hits.has(i) ? <mark key={i}>{char}</mark> : char)}</span><kbd>{command.shortcut}</kbd>
          </div>)}
        </div>)}
      </div>
      {!entries.length && <p role="status">{tr("palette.empty")}</p>}
    </div>
  </Modal>;
}
