import { useEffect, useState, useLayoutEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useAppState, useController, useLanguage, useRuntime, useT } from "../app/context";
import { activeTabOf, focusedEntry, otherWindow, visibleEntries, type WinIdx } from "../app/state";
import { formatBytes } from "../i18n";
import { baseName, crumbsOf, displayPath, joinPath, isArchivePath, parseRemotePath } from "../state/paths";
import { summary } from "../state/selection";
import { FileList } from "./FileList";
import { QuickView } from "./lister/QuickView";

import { HeaderMenu, type HeaderMenuItem } from "./HeaderMenu";
import { Icon } from "./Icon";
import { isPinned } from "../state/bookmarks";

function Tabs({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const connections = useAppState(state => state.remoteConnections);
  const windowState = useAppState((state) => state.windows[w]);
  return (
    <div className="tabs" role="tablist" aria-label={tr("pane.tabsLabel")}>
      {windowState.tabs.map((tab, index) => {
        const selected = index === windowState.activeTab;
        const remoteTab = parseRemotePath(tab.path);
        const label = remoteTab?.inner === "/" ? crumbsOf(tab.path, connections)[0]!.label : baseName(tab.path);
        return (
          <span
            key={index}
            className={selected ? "tab on" : "tab"}
            onContextMenu={(event) => { event.preventDefault(); controller.openTabMenu(w, index, event.clientX, event.clientY); }}
          >
            <button
              type="button"
              role="tab"
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              title={displayPath(tab.path, connections)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => controller.selectTab(w, index)}
            >
              {remoteTab ? <Icon name="server" className="tab-remote" /> : null}
              {tab.lockRoot ? <Icon name="pin" className="tab-remote" /> : null}
              {label}
            </button>
            {windowState.tabs.length > 1 ? (
              <button
                type="button"
                className="tab-x"
                tabIndex={-1}
                aria-label={`${tr("action.closeTab")}: ${label}`}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => controller.closeTab(w, index)}
              >
                <Icon name="close" />
              </button>
            ) : null}
          </span>
        );
      })}
      <button
        type="button"
        className="tab plus"
        tabIndex={-1}
        title={tr("tip.newTab")}
        aria-label={tr("action.newTab")}
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => controller.newTab(w)}
      >
        <Icon name="plus" />
      </button>
    </div>
  );
}

function PathInput({ w, path }: { w: WinIdx; path: string }) {
  const controller = useController();
  const tr = useT();
  const connections = useAppState(state => state.remoteConnections);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  async function onKeyDown(event: ReactKeyboardEvent<HTMLInputElement>): Promise<void> {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      controller.setPathEditing(null);
    } else if (event.key === "Enter") {
      event.preventDefault();
      // Im Archiv zeigt das Feld den lesbaren Pfad; unverändert bleibt der Archivpfad erhalten.
      const typed = event.currentTarget.value.trim();
      const ok = await controller.navigateTo(w, typed === displayPath(path, connections) ? path : typed);
      if (ok) controller.setPathEditing(null);
    }
  }
  return (
    <input
      ref={ref}
      className="path-input"
      aria-label={tr("pane.pathLabel")}
      defaultValue={displayPath(path, connections)}
      spellCheck={false}
      onKeyDown={(event) => void onKeyDown(event)}
      onBlur={() => controller.setPathEditing(null)}
    />
  );
}

function PathBar({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const connections = useAppState(state => state.remoteConnections);
  const path = useAppState((state) => activeTabOf(state, w).path);
  const filter = useAppState((state) => activeTabOf(state, w).filter);
  const backPaths = useAppState((state) => activeTabOf(state, w).back);
  const editing = useAppState((state) => state.pathEditing === w);
  const bookmarks = useAppState(state => state.settings.bookmarks);
  const pinned = isPinned(bookmarks, path);
  // Eingehängte Wechselmedien (USB u.a.) direkt als Orte; bei Fensterfokus neu laden (ein-/ausstecken).
  const [drives, setDrives] = useState<import("../ipc/types").Drive[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () => void controller.api.localDrives().then(d => { if (alive) setDrives(d); }).catch(() => {});
    load();
    window.addEventListener("focus", load);
    return () => { alive = false; window.removeEventListener("focus", load); };
  }, [controller]);
  const crumbs = crumbsOf(path, connections);
  const remote = parseRemotePath(path);
  const connection = remote ? connections[remote.id] : undefined;
  const archive = isArchivePath(path);
  const listRef = useRef<HTMLInputElement>(null);
  const crumbNav = useRef<HTMLElement>(null);
  // Lange Pfade: das Ende (aktueller Ordner) bleibt sichtbar; nach einem Ortswechsel keine alte Scrollposition behalten.
  useLayoutEffect(() => { const el = crumbNav.current; if (el) el.scrollLeft = el.scrollWidth; }, [path, editing]);
  const forwardPaths = useAppState(state => activeTabOf(state, w).forward);
  const profiles = useAppState(state => state.connections);
  const home = useAppState(state => state.home);
  const placeName = remote ? `${connection?.profile.name ?? remote.protocol} · ${(connection?.profile.protocol ?? remote.protocol).toUpperCase()}` : tr("places.computer");
  const items: HeaderMenuItem[] = [
    { kind: "heading", label: tr("places.computer") },
    { label: tr("places.computer"), icon: "computer", current: !remote, action: () => { void controller.navigatePlace(w, home); } },
    { label: tr("places.home"), icon: "home", hint: home, action: () => { void controller.navigatePlace(w, home); } },
    ...(profiles.length ? [{ kind: "heading" as const, label: tr("palette.connections") }] : []),
    ...profiles.map(entry => ({
      label: `${entry.profile.name} · ${entry.profile.protocol.toUpperCase()}`, icon: "server" as const,
      hint: tr(entry.connected ? "header.connected" : "ssh.disconnected"), connected: entry.connected,
      current: connection?.profile.id === entry.profile.id,
      action: () => { void controller.chooseConnection(w, entry.profile); },
    })),
    ...(bookmarks.length ? [{ kind: "heading" as const, label: tr("bookmarks.title") }] : []),
    ...bookmarks.map(bookmark => ({ label: bookmark.name, icon: "pin" as const, hint: displayPath(bookmark.path, connections),
      action: () => { void controller.navigatePlace(w, bookmark.path); } })),
    ...(drives.length ? [{ kind: "heading" as const, label: tr("places.drives") }] : []),
    ...drives.map(drive => ({ label: drive.name, icon: "disk" as const, hint: drive.path, current: path === drive.path,
      action: () => { void controller.navigatePlace(w, drive.path); } })),
    { label: tr("header.manageConnections"), icon: "list", action: () => controller.openDialog({ kind: "connections" }) },
    { label: tr("places.new"), icon: "plus", action: () => controller.openDialog({ kind: "connections", entry: { action: "new" } }) },
  ];

  // Strg+F aus dem globalen Handler: Filterfeld fokussieren.
  useEffect(() => {
    const focusFilter = (event: Event) => {
      if ((event as CustomEvent<WinIdx>).detail === w) listRef.current?.focus();
    };
    document.addEventListener("dualfile:focus-filter", focusFilter);
    return () => document.removeEventListener("dualfile:focus-filter", focusFilter);
  }, [w]);

  return (
    <div className="crumbs">
      <button type="button" className="header-icon" aria-label={tr("action.back")} title={`${tr("action.back")} (Alt+←)`} disabled={!backPaths.length} onClick={() => void controller.goBack(w)}><Icon name="back" /></button>
      <button type="button" className="header-icon" aria-label={tr("action.forward")} title={`${tr("action.forward")} (Alt+→)`} disabled={!forwardPaths.length} onClick={() => void controller.goForward(w)}><Icon name="forward" /></button>
      <button type="button" className="header-icon" aria-label={tr("action.up")} title={tr("tip.up")} disabled={path === "/"} onClick={() => void controller.goUp(w)}><Icon name="up" /></button>
      <button type="button" className="header-icon" aria-label={tr("action.reload")} title={tr("tip.reload")} onClick={() => void controller.reload(w)}><Icon name="reload" /></button>
      <HeaderMenu label={`${tr("places.choose")}: ${placeName}`} title={connection ? `${placeName} · ${connection.endpoint}` : placeName} className="place-menu" portal items={items}>
        <span aria-hidden="true" className={`connection-dot ${!remote || connection?.connected ? "connected" : "disconnected"}`} />
        <Icon name={remote ? "server" : "computer"} /><span className="place-name">{placeName}</span>
      </HeaderMenu>
      {archive ? (
        <span className="chip" title={tr("archive.chipTitle")}>
          {tr("archive.chip")}
        </span>
      ) : null}
      {editing ? (
        <PathInput w={w} path={path} />
      ) : (
        <nav
          ref={crumbNav}
          key={remote ? remote.root : archive ? "archive" : "local"}
          className="crumb-path"
          aria-label={tr("pane.pathLabel")}
          title={tr("tip.pathEdit")}
          onClick={(event) => {
            if (event.target === event.currentTarget) controller.setPathEditing(w);
          }}
        >
          {crumbs.map((crumb, index) => (
            <span key={crumb.path} className="crumb">
              {(remote ? index > 0 : index > 1) ? <Icon name="right" size={12} className="sep" /> : null}
              <button
                type="button"
                tabIndex={-1}
                aria-current={index === crumbs.length - 1 ? "location" : undefined}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => void controller.navigateTo(w, crumb.path)}
              >
                {crumb.label}
              </button>
            </span>
          ))}
        </nav>
      )}
      <button type="button" className="header-icon pin" aria-pressed={pinned} aria-label={tr(pinned ? "bookmarks.unpin" : "bookmarks.pin")} title={`${tr(pinned ? "bookmarks.unpin" : "tip.pin")} (Ctrl+D)`} onClick={() => controller.togglePin(w)}><Icon name="pin" /></button>
      <div className="panel-filter">
      <input
        ref={listRef}
        className="filter"
        type="search"
        value={filter}
        placeholder={tr("pane.filterPlaceholder")}
        aria-label={tr("tip.filter")}
        onFocus={() => controller.setActive(w)}
        onChange={(event) => controller.setFilter(w, event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            controller.setFilter(w, "");
            document.querySelector<HTMLElement>(`[data-pane="${w}"] [role="grid"]`)?.focus();
          } else if (event.key === "Enter" || event.key === "ArrowDown") {
            event.preventDefault();
            event.stopPropagation();
            document.querySelector<HTMLElement>(`[data-pane="${w}"] [role="grid"]`)?.focus();
          }
        }}
      />
      {filter && <button type="button" aria-label={tr("header.clearFilter")} onClick={() => { controller.setFilter(w, ""); listRef.current?.focus(); }}><Icon name="close" size={12} /></button>}
      <kbd>/</kbd>
      </div>
    </div>
  );
}

export function StatusBar({ w }: { w: WinIdx }) {
  const tr = useT();
  const language = useLanguage();
  const entries = useAppState((state) => visibleEntries(state, w));
  const selection = useAppState((state) => activeTabOf(state, w).selection);
  const showHidden = useAppState((state) => state.showHidden);
  const { api } = useRuntime();
  const path = useAppState(state => activeTabOf(state, w).path);
  const remote = parseRemotePath(path) !== null;
  const listing = useAppState(state => state.listings[w]);
  const [space, setSpace] = useState<{ path: string; available: number } | null>(null);
  useEffect(() => {
    let dead = false;
    const timer = setTimeout(() => {
      void api.spaceFree(path).then(value => {
        if (!dead) setSpace(value ? { path, available: value.available } : null);
      }, () => { if (!dead) setSpace(null); });
    }, 100);
    return () => { dead = true; clearTimeout(timer); };
  }, [api, path, listing]);
  const available = space?.path === path ? space.available : null;
  const info = summary(selection, entries);
  return (
    <div className="foot" role="status">
      <span className="tabular-nums"><span>{tr("status.selected", { selected: info.selectedCount, total: info.totalCount })}</span> · <span>{tr(entries.some(entry => entry.kind === "dir" && selection.selected.has(entry.name)) ? "status.selectedFileSize" : "status.selectedSize", { size: formatBytes(info.selectedBytes, language) })}</span></span>
      <span className="status-context tabular-nums">{tr(showHidden ? "status.hiddenShown" : "status.hiddenHidden")}{available !== null ? ` · ${tr("status.free", { size: formatBytes(available, language) })}` : remote ? ` · ${tr("status.remote")}` : ""}</span>
    </div>
  );
}

export function Pane({ w }: { w: WinIdx }) {
  const controller = useController();
  const tr = useT();
  const active = useAppState((state) => state.active === w);
  const dropTarget = useAppState((state) => state.drag?.over === w);
  // Schnellansicht: das Fenster, das nicht aktiv ist, zeigt die Vorschau statt seiner Liste.
  const previewPath = useAppState(state => {
    const source = otherWindow(w);
    const folder = activeTabOf(state, source).path;
    const entry = focusedEntry(state, source);
    return entry && entry.name !== ".." ? joinPath(folder, entry.name) : folder;
  });
  const preview = useAppState((state) => state.quickView && state.active !== w);
  return (
    <section
      className={`pane${active ? " active" : ""}${dropTarget ? " drop" : ""}`}
      data-pane={w}
      aria-label={tr(w === 0 ? "pane.left" : "pane.right")}
      onMouseDown={() => controller.setActive(w)}
      onDragOver={(event) => {
        event.preventDefault();
        if (event.dataTransfer) event.dataTransfer.dropEffect = event.shiftKey ? "move" : "copy";
        controller.dragOver(w, event.shiftKey, event.clientX, event.clientY);
      }}
      onDrop={(event) => {
        event.preventDefault();
        controller.drop(w, event.shiftKey);
      }}
    >
      <Tabs w={w} />
      {preview ? <div className="crumbs quick-path" title={displayPath(previewPath)}>{displayPath(previewPath)}</div> : <PathBar w={w} />}
      {preview ? (
        <QuickView source={otherWindow(w)} />
      ) : (
        <>
          <FileList w={w} />
          <StatusBar w={w} />
        </>
      )}
    </section>
  );
}
