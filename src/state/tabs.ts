import type { TabState, WindowSession } from "../ipc/types";
import { normalizePath, parentPath } from "./paths";
import { createSelection, type SelectionState } from "./selection";

/** Ein Tab: persistierbarer Teil (`TabState`) plus flüchtiger Zustand (Auswahl, Verlauf). */
export interface Tab extends TabState {
  selection: SelectionState;
  back: readonly string[];
  forward: readonly string[];
}

export interface WindowState {
  tabs: readonly Tab[];
  activeTab: number;
}

export function createTab(path: string, sort = "name", filter = "", lockRoot?: string): Tab {
  const tab: Tab = { path: normalizePath(path), sort, filter, selection: createSelection(), back: [], forward: [] };
  if (lockRoot) tab.lockRoot = normalizePath(lockRoot);
  return tab;
}

/** A locked tab may only reach its lock root and folders below it. */
function withinLock(tab: Tab, target: string): boolean {
  if (!tab.lockRoot) return true;
  const root = tab.lockRoot;
  return target === root || target.startsWith(root === "/" ? "/" : root + "/");
}

/** Toggle the folder lock of the tab at `index`: lock to its current folder, or unlock. */
export function toggleLock(w: WindowState, index: number): WindowState {
  const tab = w.tabs[index];
  if (!tab) return w;
  const next = { ...tab };
  if (tab.lockRoot) delete next.lockRoot;
  else next.lockRoot = tab.path;
  const tabs = w.tabs.slice();
  tabs[index] = next;
  return { ...w, tabs };
}

export const createWindow = (path: string): WindowState => ({ tabs: [createTab(path)], activeTab: 0 });

export const activeTab = (w: WindowState): Tab => w.tabs[w.activeTab] ?? w.tabs[0]!;

function replaceActive(w: WindowState, update: (tab: Tab) => Tab): WindowState {
  return { ...w, tabs: w.tabs.map((tab, i) => (i === w.activeTab ? update(tab) : tab)) };
}

/** Strg+T: Kopie des aktiven Tabs (samt Verlauf und Auswahl), direkt daneben, wird aktiv. */
export function newTab(w: WindowState): WindowState {
  const copy: Tab = { ...activeTab(w) };
  const tabs = [...w.tabs.slice(0, w.activeTab + 1), copy, ...w.tabs.slice(w.activeTab + 1)];
  return { tabs, activeTab: w.activeTab + 1 };
}

/**
 * Strg+W. Der letzte Tab bleibt – ein Fenster ohne Tab hätte keinen Pfad. Schließt man einen Tab
 * vor dem aktiven, wandert der Index mit, damit derselbe Tab aktiv bleibt.
 */
export function closeTab(w: WindowState, index: number = w.activeTab): WindowState {
  if (w.tabs.length <= 1 || index < 0 || index >= w.tabs.length) return w;
  const tabs = w.tabs.filter((_, i) => i !== index);
  const active = index < w.activeTab ? w.activeTab - 1 : Math.min(w.activeTab, tabs.length - 1);
  return { tabs, activeTab: active };
}

export function closeOthers(w: WindowState, keep: number): WindowState {
  if (keep < 0 || keep >= w.tabs.length || w.tabs.length <= 1) return w;
  return { tabs: [w.tabs[keep]!], activeTab: 0 };
}

export const nextTab = (w: WindowState): WindowState => ({ ...w, activeTab: (w.activeTab + 1) % w.tabs.length });
export const prevTab = (w: WindowState): WindowState => ({
  ...w,
  activeTab: (w.activeTab - 1 + w.tabs.length) % w.tabs.length,
});
export function selectTab(w: WindowState, index: number): WindowState {
  return index >= 0 && index < w.tabs.length ? { ...w, activeTab: index } : w;
}

/**
 * Wechselt den Ordner des aktiven Tabs. Der Filter gilt nur für den Ordner, in dem er getippt
 * wurde, und die Auswahl (Namen) wäre im neuen Ordner bedeutungslos – beides wird zurückgesetzt.
 * Derselbe Pfad ist kein Schritt im Verlauf.
 */
export function navigate(w: WindowState, path: string): WindowState {
  const target = normalizePath(path);
  return replaceActive(w, (tab) =>
    target === tab.path || !withinLock(tab, target)
      ? tab
      : { ...tab, path: target, filter: "", selection: createSelection(), back: [...tab.back, tab.path], forward: [] },
  );
}

/** Rücktaste: eine Ebene hoch; an der Wurzel bleibt es "/" (kein Verlaufseintrag). */
export function goUp(w: WindowState): WindowState {
  return navigate(w, parentPath(activeTab(w).path));
}

export function back(w: WindowState): WindowState {
  return replaceActive(w, (tab) => {
    const previous = tab.back[tab.back.length - 1];
    if (previous === undefined) return tab;
    return {
      ...tab,
      path: previous,
      filter: "",
      selection: createSelection(),
      back: tab.back.slice(0, -1),
      forward: [...tab.forward, tab.path],
    };
  });
}

export function forward(w: WindowState): WindowState {
  return replaceActive(w, (tab) => {
    const next = tab.forward[tab.forward.length - 1];
    if (next === undefined) return tab;
    return {
      ...tab,
      path: next,
      filter: "",
      selection: createSelection(),
      back: [...tab.back, tab.path],
      forward: tab.forward.slice(0, -1),
    };
  });
}

/** Resolve a history target to its current connection ID without adding a navigation step. */
export function resolveHistory(w: WindowState, direction: "back" | "forward", path: string): WindowState {
  const moved = direction === "back" ? back(w) : forward(w);
  return replaceActive(moved, tab => ({ ...tab, path: normalizePath(path) }));
}

export const setSort = (w: WindowState, sort: string): WindowState => replaceActive(w, (tab) => ({ ...tab, sort }));
export const setFilter = (w: WindowState, filter: string): WindowState => replaceActive(w, (tab) => ({ ...tab, filter }));
export const setSelection = (w: WindowState, selection: SelectionState): WindowState =>
  replaceActive(w, (tab) => ({ ...tab, selection }));

// Direct server addresses may carry user and host names and are refused by the core when
// saving; saved connections (`conn:`) and local folders are kept.
const UNSAVED_PATH = /^(ftp|sftp|smb|webdav|plugin|trash):/;

export function toSession(w: WindowState): WindowSession {
  const kept = w.tabs.filter((tab) => !UNSAVED_PATH.test(tab.path));
  const active = kept.indexOf(w.tabs[w.activeTab]!);
  return {
    tabs: kept.map(({ path, sort, filter, lockRoot }) => (lockRoot ? { path, sort, filter, lockRoot } : { path, sort, filter })),
    activeTab: active < 0 ? 0 : active,
  };
}

/**
 * Sitzung → Zustand. Die Einstellungsdatei kann von Hand verändert sein: kein Tab ergibt einen
 * Tab auf `fallbackPath`, ein ungültiger Aktiv-Index wird auf den ersten Tab gesetzt.
 */
export function fromSession(session: WindowSession, fallbackPath: string): WindowState {
  const tabs = session.tabs.map((t) => createTab(t.path || fallbackPath, t.sort, t.filter, t.lockRoot));
  if (tabs.length === 0) return createWindow(fallbackPath);
  const valid = Number.isInteger(session.activeTab) && session.activeTab >= 0 && session.activeTab < tabs.length;
  return { tabs, activeTab: valid ? session.activeTab : 0 };
}
