import type { Controller } from "./controller";
import { activeTabOf, focusedEntry, visibleEntries, type AppState } from "./state";
import type { IconName } from "../components/Icon";
import type { TranslationKey } from "../i18n";
import type { Command } from "../state/keymap";
import { isArchivePath } from "../state/paths";

export type ToolKind = "usage" | "dupes" | "similar";
export interface PaletteCommand {
  id: string;
  title: string;
  icon: IconName;
  shortcut: string;
  group: "actions" | "bookmarks" | "connections" | "tabs";
  enabled: (state: AppState) => boolean;
  execute: () => void;
}
const ready = (s: AppState) => !s.listings[s.active].loading && !s.listings[s.active].error;
const writable = (s: AppState) => ready(s) && !isArchivePath(activeTabOf(s, s.active).path);
const selected = (s: AppState) => {
  const selection = activeTabOf(s, s.active).selection;
  return ready(s) && visibleEntries(s, s.active).some((e, i) => e.name !== ".." && (selection.selected.size ? selection.selected.has(e.name) : i === selection.focus));
};
export function createCommands(state: AppState, controller: Controller, tr: (key: TranslationKey) => string, openTool: (kind: ToolKind) => void): PaletteCommand[] {
  const result: PaletteCommand[] = [];
  const add = (id: string, key: TranslationKey, icon: IconName, shortcut: string, execute: () => void, enabled: PaletteCommand["enabled"] = () => true) => {
    result.push({ id, title: tr(key), icon, shortcut, group: "actions", execute, enabled });
  };
  const command = (id: Command, key: TranslationKey, icon: IconName, shortcut: string, enabled?: PaletteCommand["enabled"]) => add(id, key, icon, shortcut, () => controller.command(id), enabled);
  command("goBack", "action.back", "back", "Alt+←", s => activeTabOf(s, s.active).back.length > 0);
  command("goForward", "action.forward", "forward", "Alt+→", s => activeTabOf(s, s.active).forward.length > 0);
  command("swapPanes", "menu.view.swapPanes", "swap", "Ctrl+U");
  command("toggleHidden", "menu.view.hidden", "hidden", "Ctrl+H");
  command("quickView", "menu.view.quickView", "preview", "Ctrl+Q");
  add("mirror", "menu.view.mirror", "couple", "", () => controller.setMirror(!state.settings.mirror));
  command("newTab", "action.newTab", "plus", "Ctrl+T");
  command("closeTab", "action.closeTab", "close", "Ctrl+W", s => s.windows[s.active].tabs.length > 1);
  command("copy", "action.copy", "copy", "F5", s => selected(s) && !isArchivePath(activeTabOf(s, s.active === 0 ? 1 : 0).path));
  command("move", "action.move", "swap", "F6", s => selected(s) && writable(s) && !isArchivePath(activeTabOf(s, s.active === 0 ? 1 : 0).path));
  command("trash", "action.trash", "close", "F8", s => selected(s) && writable(s));
  command("rename", "action.rename", "edit", "F2", s => writable(s) && !!focusedEntry(s, s.active) && focusedEntry(s, s.active)?.name !== "..");
  command("makeDir", "undoLabel.mkdir", "dir", "F7", writable);
  command("multiRename", "action.multiRename", "rename", "Ctrl+M", s => selected(s) && writable(s));
  command("compareDirs", "action.compareDirs", "compare", "Ctrl+Shift+D");
  add("usage", "space.button", "disk", "", () => openTool("usage"));
  add("dupes", "dupes.title", "copy", "", () => openTool("dupes"));
  add("similar", "similar.title", "images", "", () => openTool("similar"));
  add("history", "undo.history", "history", "Ctrl+Shift+Z", () => controller.openDialog({ kind: "history" }));
  add("settings", "menu.view.settings", "settings", "Ctrl+,", () => controller.openDialog({ kind: "settings" }));
  add("search", "search.title", "search", "", () => controller.openDialog({ kind: "search", w: state.active }));
  add("connections", "header.manageConnections", "connect", "Ctrl+N", () => controller.openDialog({ kind: "connections" }));
  for (const [action, key] of [["new", "ssh.new"], ["import", "ssh.import"], ["system", "system.import"]] as const) {
    add(action, key, "import", "", () => controller.openDialog({ kind: "connections", entry: { action } }));
  }
  for (const theme of ["schiefer", "papier", "werkstatt"] as const) {
    add(`theme:${theme}`, `settings.theme.${theme}`, "settings", "", () => controller.updateSettings(s => ({ ...s, theme })));
    result[result.length - 1]!.title = `${tr("palette.theme")}: ${tr(`settings.theme.${theme}`)}`;
  }
  for (const bookmark of state.settings.bookmarks) result.push({
    id: `bookmark:${bookmark.path}`, title: `${tr("palette.bookmark")}: ${bookmark.name}`, icon: "pin", shortcut: "", group: "bookmarks", enabled: () => true,
    execute: () => { void controller.openBookmark(bookmark.path); },
  });
  for (const { profile, connected } of state.connections) result.push({
    id: `connection:${profile.id}:${connected}`, title: `${tr(connected ? "ssh.disconnect" : "ssh.connect")}: ${profile.name}`, icon: connected ? "disconnect" : "connect", shortcut: "", group: "connections", enabled: () => true,
    execute: () => controller.openDialog({ kind: "connections", entry: { action: connected ? "disconnect" : "connect", profile } }),
  });
  for (const w of [0, 1] as const) state.windows[w].tabs.forEach((tab, index) => result.push({
    id: `tab:${w}:${index}:${tab.path}`, title: `${tr("palette.tab")}: ${tab.path} (${w + 1} · ${index + 1})`, icon: "dir", shortcut: "", group: "tabs", enabled: () => true,
    execute: () => { controller.setActive(w); controller.selectTab(w, index); },
  }));
  return result;
}

/** Token/subsequence search, with character offsets preserved for highlighting. */
export function matchCommand(title: string, query: string): Set<number> | null {
  const fold = (s: string) => s.toLowerCase().normalize("NFD").replace(/\p{M}/gu, "").replace(/ß/g, "ss").replace(/ae/g, "a").replace(/oe/g, "o").replace(/ue/g, "u");
  const chars: string[] = [], offsets: number[] = [];
  Array.from(title).forEach((char, i) => { for (const c of fold(char)) { chars.push(c); offsets.push(i); } });
  const text = chars.join("");
  const hits = new Set<number>();
  for (const token of fold(query).trim().split(/\s+/).filter(Boolean)) {
    let pos = 0;
    for (const char of token) {
      const found = text.indexOf(char, pos);
      if (found < 0) return null;
      hits.add(offsets[found]!); pos = found + 1;
    }
  }
  return hits;
}
