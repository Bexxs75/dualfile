import { useEffect, useRef, type KeyboardEvent } from "react";
import { useAppState, useController, useT } from "../app/context";
import type { TranslationKey } from "../i18n";
import { isArchivePath } from "../state/paths";
import { activeTabOf, otherWindow, visibleEntries } from "../app/state";

export function ContextMenu() {
  const menu = useAppState((state) => state.menu);
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

  if (!menu) return null;
  const run = (action: () => void) => () => {
    controller.closeMenu();
    action();
  };
  const pinTarget = menu.tab === undefined && !menu.background ? controller.bookmarkTarget(menu.w) : null;
  const items: { text: TranslationKey; action: () => void; disabled?: boolean }[] = menu.tab !== undefined ? [
    { text: tabLocked ? "action.unlockTab" : "action.lockTab", action: () => controller.toggleTabLock(menu.w, menu.tab!) },
    { text: "action.newTab", action: () => controller.newTab(menu.w) },
    { text: "action.closeTab", action: () => controller.closeTab(menu.w, menu.tab), disabled: tabCount <= 1 },
    { text: "action.closeOtherTabs", action: () => controller.closeOtherTabs(menu.w, menu.tab!), disabled: tabCount <= 1 },
  ] : menu.background ? [
    { text: "action.newFolder", action: () => controller.command("makeDir"), disabled: readOnly },
    { text: "action.refresh", action: () => controller.command("refresh") },
    { text: "menu.view.hidden", action: () => controller.command("toggleHidden") },
    { text: "context.compareDirs", action: () => controller.openCompare() },
  ] : [
    { text: "context.open", action: () => controller.command("open"), disabled: multiple },
    { text: "context.copyPath", action: () => void controller.copyPath(menu.w) },
    { text: "context.rename", action: () => controller.openRename(), disabled: readOnly || multiple },
    ...(packable ? [{ text: "context.pack" as const, action: () => controller.openPack(menu.w) }] : []),
    ...(controller.canMultiRename(menu.w) ? [{ text: "context.multiRename" as const, action: () => controller.openMultiRename(menu.w) }] : []),
    ...(controller.diffTargets() ? [{ text: "context.compareFiles" as const, action: () => controller.openDiffFromPanes() }] : []),
    ...(pinTarget ? [pinTarget && bookmarks.some(b => b.path === pinTarget)
      ? { text: "context.unpinPlace" as const, action: () => controller.unpinBookmark(pinTarget) }
      : { text: "context.pinPlace" as const, action: () => controller.pinBookmarkPath(pinTarget) }] : []),
    { text: "context.trash", action: () => void controller.trashSelection(), disabled: readOnly },
    { text: "context.properties", action: () => controller.openProperties(menu.w), disabled: multiple },
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

  const left = Math.max(4, Math.min(menu.x, window.innerWidth - 200));
  const top = Math.max(4, Math.min(menu.y, window.innerHeight - 310));
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
        style={{ left, top }}
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        {items.map((item) => (
          <button
            key={item.text}
            type="button"
            role="menuitem"
            disabled={item.disabled}
            title={item.disabled ? tr("error.readOnly") : undefined}
            onClick={run(item.action)}
          >
            {tr(item.text)}
          </button>
        ))}
      </div>
    </div>
  );
}
