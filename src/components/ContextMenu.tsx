import { useEffect, useLayoutEffect, useId, useRef, type KeyboardEvent } from "react";
import { useAppState, useController, useLanguage, useT } from "../app/context";
import { formatBytes, type TranslationKey } from "../i18n";
import { isArchivePath, parseRemotePath } from "../state/paths";
import { activeTabOf, focusedEntry, otherWindow, visibleEntries } from "../app/state";

import { menuShortcut, type Command } from "../state/keymap";

export function ContextMenu() {
  const state = useAppState(state => state);
  const menu = state.menu;
  const language = useLanguage();
  const id = useId();
  const controller = useController();
  const tr = useT();
  const ref = useRef<HTMLDivElement>(null);
  const readOnly = useAppState((state) => (state.menu ? isArchivePath(activeTabOf(state, state.menu.w).path) : false));
  const packable = useAppState((state) => (state.menu ? !isArchivePath(activeTabOf(state, otherWindow(state.menu.w)).path) : false));

  const multiple = useAppState(state => {
    if (!state.menu) return false;
    const w = state.menu.w;
    return visibleEntries(state, w).filter(entry => activeTabOf(state, w).selection.selected.has(entry.name)).length > 1;
  });
  const bookmarks = useAppState(state => state.settings.bookmarks);
  const tabCount = useAppState(state => (state.menu ? state.windows[state.menu.w].tabs.length : 0));
  const tabLocked = useAppState(state => (state.menu?.tab !== undefined ? Boolean(state.windows[state.menu.w].tabs[state.menu.tab]?.lockRoot) : false));

  useEffect(() => {
    if (!menu) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    ref.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    return () => {
      if (previous && document.contains(previous)) previous.focus();
    };
  }, [menu]);

  useLayoutEffect(() => {
    if (!menu || !ref.current) return;
    const place = () => {
      const el = ref.current!;
      el.style.left = `${Math.max(4, Math.min(menu.x, window.innerWidth - el.offsetWidth - 4))}px`;
      el.style.top = `${Math.max(4, Math.min(menu.y, window.innerHeight - el.offsetHeight - 4))}px`;
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [menu, state, language]);

  if (!menu) return null;
  const run = (action: () => void) => () => {
    controller.closeMenu();
    action();
  };
  const pinTarget = menu.tab === undefined && !menu.background ? controller.bookmarkTarget(menu.w) : null;
  const path = activeTabOf(state, menu.w).path;
  const entry = focusedEntry(state, menu.w);
  const parent = entry?.name === "..";
  const count = visibleEntries(state, menu.w).filter(e => e.name !== ".." && activeTabOf(state, menu.w).selection.selected.has(e.name)).length;
  const archiveReason = readOnly ? "menu.reason.archive" : undefined;
  const singleReason = multiple ? "menu.reason.single" : undefined;
  const destinationReason: TranslationKey | undefined = !packable ? "menu.reason.destination" : undefined;
  type Item = { text: TranslationKey; action: () => void; reason?: TranslationKey; command?: Command | "pin"; danger?: boolean; checked?: boolean };
  const cmd = (text: TranslationKey, command: Command, reason?: TranslationKey): Item => ({ text, command, reason, action: () => controller.command(command) });
  const groups: Item[][] = menu.tab !== undefined ? [[
    { text: tabLocked ? "action.unlockTab" : "action.lockTab", action: () => controller.toggleTabLock(menu.w, menu.tab!) },
    { text: "action.newTab", command: "newTab", action: () => controller.newTab(menu.w) },
    { text: "action.closeTab", command: "closeTab", action: () => controller.closeTab(menu.w, menu.tab), reason: tabCount <= 1 ? "menu.reason.lastTab" : undefined },
    { text: "action.closeOtherTabs", action: () => controller.closeOtherTabs(menu.w, menu.tab!), reason: tabCount <= 1 ? "menu.reason.lastTab" : undefined },
  ]] : menu.background ? [
    [cmd("action.newFolder", "makeDir", archiveReason)],
    [cmd("action.refresh", "refresh"), { ...cmd("menu.view.hidden", "toggleHidden"), checked: state.showHidden }],
    [cmd("keys.cmd.selectAll", "selectAll"), cmd("action.compareDirs", "compareDirs")],
    [{ text: "space.button", action: () => controller.openTool("usage", path) }, { text: "dupes.title", action: () => controller.openTool("dupes", path) }],
  ] : parent ? [[cmd("context.open", "open")]] : [
    [cmd("context.open", "open", singleReason)],
    !multiple && entry?.kind !== "dir" ? [cmd("fkey.f3", "view"), cmd("fkey.f4", "edit", archiveReason)] : [],
    [cmd("action.copy", "copy", destinationReason), cmd("action.move", "move", archiveReason ?? destinationReason),
      ...(packable ? [{ text: "context.pack" as const, command: "pack" as const, action: () => controller.openPack(menu.w) }] : []),
      ...(readOnly ? [{ text: "context.extract" as const, action: () => controller.openTransfer("copy", menu.w), reason: destinationReason }] : [])],
    [cmd("action.rename", "rename", archiveReason ?? (multiple ? "menu.reason.multiRename" : undefined)),
      ...(controller.canMultiRename(menu.w) ? [cmd("action.multiRename", "multiRename")] : []),
      ...(controller.diffTargets() ? [cmd("context.compareFiles", "compareFiles")] : [])],
    [{ text: "context.copyPath", action: () => void controller.copyPath(menu.w) },
      ...(pinTarget ? [{ text: bookmarks.some(b => b.path === pinTarget) ? "context.unpinPlace" as const : "context.pinPlace" as const, command: "pin" as const,
        action: () => { if (bookmarks.some(b => b.path === pinTarget)) controller.unpinBookmark(pinTarget); else controller.pinBookmarkPath(pinTarget); } }] : []),
      { text: "context.properties", action: () => controller.openProperties(menu.w), reason: singleReason }],
    [cmd("action.trash", "trash", archiveReason ?? (parseRemotePath(path) ? "menu.reason.serverTrash" : undefined))],
    [{ ...cmd("action.deletePermanent", "deletePermanent", archiveReason), danger: true }],
  ];

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const buttons = [...(ref.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? [])];
    const index = buttons.indexOf(document.activeElement as HTMLElement);
    if (event.key === "ArrowDown") buttons[(index + 1) % buttons.length]?.focus();
    else if (event.key === "ArrowUp") buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
    else if (event.key === "Home") buttons[0]?.focus();
    else if (event.key === "End") buttons[buttons.length - 1]?.focus();
    else if (event.key === "Escape" || event.key === "Tab") controller.closeMenu();
    else return;
    event.preventDefault();
    event.stopPropagation();
  }


  return (
    <div
      className="menu-backdrop"
      onMouseDown={() => controller.closeMenu()}
      onContextMenu={(event) => {
        event.preventDefault();
        controller.closeMenu();
      }}
    >
      <div
        ref={ref}
        className="ctx"
        role="menu"
        aria-label={tr("ui.contextMenu")}
        style={{ left: menu.x, top: menu.y }}
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        {menu.tab === undefined && !menu.background && entry && <div className="ctx-heading">
          <strong>{multiple ? tr("menu.targets", { count }) : entry.name}</strong>
          <small>{multiple ? tr("menu.selection") : `${tr(entry.kind === "dir" ? "type.folder" : "type.file")} · ${formatBytes(entry.size, language)}`}</small>
        </div>}
        {groups.filter(group => group.length).map((group, index) => <div className="ctx-group" key={index}>
          {index > 0 && <div role="separator" className="header-menu-separator" />}
          {group.map(item => <button key={item.text} type="button"
            role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"} aria-checked={item.checked}
            aria-label={tr(item.text)} aria-describedby={item.reason ? `${id}-${item.text}` : undefined}
            disabled={!!item.reason} aria-disabled={!!item.reason} className={item.danger ? "danger" : undefined}
            onClick={run(item.action)}>
            <span className="ctx-label">{item.checked !== undefined && <span aria-hidden="true">{item.checked ? "✓ " : ""}</span>}{tr(item.text)}</span>
            {item.command && <kbd>{menuShortcut(item.command, state.settings.keybindings, language)}</kbd>}
            {item.reason && <small id={`${id}-${item.text}`} className="menu-reason">{tr(item.reason)}</small>}
          </button>)}
        </div>)}
      </div>
    </div>
  );
}
