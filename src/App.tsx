import { SidebarLayout } from "./components/Sidebar";
import { SystemUpdates } from "./components/dialogs/SystemDialogs";
import { JobDialogs } from "./components/dialogs/JobDialogs";
import { SshPromptDialogs } from "./components/dialogs/SshPromptDialog";
import { ConnectionsDialog } from "./components/dialogs/ConnectionsDialog";
import { useEffect, useMemo } from "react";
import { RuntimeProvider, useAppState, useController, useLanguage, useRuntime, useT } from "./app/context";
import { useGlobalKeys } from "./app/keyboard";
import { createRuntime, type AppRuntime } from "./app/runtime";
import type { ControllerOptions } from "./app/controller";
import { ConflictDialog } from "./components/dialogs/ConflictDialog";
import { ConfirmTransferDialog } from "./components/dialogs/ConfirmTransferDialog";
import { BookmarkDialog } from "./components/dialogs/BookmarkDialog";
import { DeleteDialog } from "./components/dialogs/DeleteDialog";
import { PackDialog } from "./components/dialogs/PackDialog";
import { CompareDialog } from "./components/dialogs/CompareDialog";
import { DiffDialog } from "./components/dialogs/DiffDialog";
import { SyncDialog } from "./components/dialogs/SyncDialog";
import { ArchiveErrorDialog } from "./components/dialogs/ArchiveErrorDialog";
import { MakeDirDialog, RenameDialog } from "./components/dialogs/NameDialog";
import { MultiRenameDialog } from "./components/dialogs/MultiRenameDialog";
import { PatternDialog } from "./components/dialogs/PatternDialog";
import { PropertiesDialog } from "./components/dialogs/PropertiesDialog";
import { LogDialog } from "./components/dialogs/LogDialog";
import { WhatsNewDialog } from "./components/dialogs/WhatsNewDialog";
import { SettingsDialog } from "./components/dialogs/SettingsDialog";
import { SearchDialog } from "./components/dialogs/SearchDialog";
import { ContextMenu } from "./components/ContextMenu";
import { DragHint } from "./components/DragHint";
import { FKeys } from "./components/FKeys";
import { TerminalPanel } from "./components/TerminalPanel";
import { activeTabOf } from "./app/state";
import { JobsPanel } from "./components/JobsPanel";
import { NoticeBar } from "./components/NoticeBar";
import { Lister } from "./components/lister/Lister";
import { Pane } from "./components/Pane";
import { Ribbon } from "./components/Ribbon";
import { UndoHistory } from "./components/UndoHistory";
import { Toasts } from "./components/Toasts";
import { Toolbar } from "./components/Toolbar";
import type { DualfileApi } from "./ipc/api";
import { applyTheme } from "./themes/tokens";

function Dialogs({ api }: { api: DualfileApi }) {
  const dialog = useAppState((state) => state.dialog);
  const controller = useController();
  const conflict = useAppState((state) => state.conflicts[0]);
  const lister = useAppState((state) => state.lister);
  const compare = useAppState((state) => state.compare);
  const diff = useAppState((state) => state.diff);
  return (
    <>
      {dialog?.kind === "transfer" ? <ConfirmTransferDialog plan={dialog.plan} /> : null}
      {dialog?.kind === "delete" ? <DeleteDialog model={dialog.model} reason={dialog.reason} /> : null}
      {dialog?.kind === "rename" ? <RenameDialog name={dialog.name} existing={dialog.existing} /> : null}
      {dialog?.kind === "multiRename" ? <MultiRenameDialog dialog={dialog} /> : null}
      {dialog?.kind === "makeDir" ? <MakeDirDialog existing={dialog.existing} /> : null}
      {dialog?.kind === "pack" ? (
        <PackDialog sourceNames={dialog.sourceNames} destination={dialog.destination} existing={dialog.existing} suggestion={dialog.suggestion} />
      ) : null}
      {dialog?.kind === "archiveError" ? <ArchiveErrorDialog path={dialog.path} message={dialog.message} external={dialog.external} /> : null}
      {dialog?.kind === "properties" ? <PropertiesDialog entry={dialog.entry} path={dialog.path} /> : null}
      {dialog?.kind === "pattern" ? <PatternDialog mode={dialog.mode} w={dialog.w} /> : null}
      {dialog?.kind === "history" ? <UndoHistory /> : null}
      {dialog?.kind === "connections" ? <ConnectionsDialog entry={dialog.entry} api={api} onClose={() => controller.closeDialog()} /> : null}
      {dialog?.kind === "whatsNew" ? <WhatsNewDialog versions={dialog.versions} /> : null}
      {dialog?.kind === "settings" ? <SettingsDialog /> : null}
      {dialog?.kind === "log" ? <LogDialog /> : null}
      {dialog?.kind === "search" ? <SearchDialog w={dialog.w} /> : null}
      {dialog?.kind === "bookmark" ? <BookmarkDialog path={dialog.path} /> : null}
      {lister ? <Lister w={lister.w} /> : null}
      {compare ? <CompareDialog session={compare} /> : null}
      {compare?.sync ? <SyncDialog session={compare} sync={compare.sync} /> : null}
      {diff ? <DiffDialog session={diff} /> : null}
      <JobDialogs />
      {conflict ? <ConflictDialog key={`${conflict.jobId}:${conflict.destinationPath}`} conflict={conflict} /> : null}
    </>
  );
}

function Shell({ api }: { api: DualfileApi }) {
  const { store, controller, system, settingsWindow } = useRuntime();
  const tr = useT();
  const language = useLanguage();
  const terminal = useAppState(state => state.terminal);
  const terminalCwd = useAppState(state => activeTabOf(state, state.active).path);
  const phase = useAppState((state) => state.phase);
  const theme = useAppState((state) => state.settings.theme);
  const accentAutomatic = useAppState((state) => state.settings.accentAutomatic);
  const accent = useAppState((state) => state.settings.accent);
  const density = useAppState((state) => state.settings.density);
  const fontSize = useAppState((state) => state.settings.fontSize);

  useEffect(() => {
    let mounted = true;
    let stopBridge: (() => void) | undefined;
    void controller.start().then(() => {
      if (mounted && store.getState().phase === "ready") { void system.start(controller.language()); stopBridge = settingsWindow.start(); }
    });
    return () => { mounted = false; stopBridge?.(); controller.dispose(); };
  }, [controller, store, system, settingsWindow]);
  useGlobalKeys(store, controller);
  useEffect(() => applyTheme(document.documentElement, { theme, accent, accentAutomatic, density, fontSize }), [theme, accent, accentAutomatic, density, fontSize]);
  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);

  if (phase !== "ready") return <main className="app" aria-label={tr("app.title")} aria-busy="true" />;
  return (
    <main className="app" aria-label={tr("app.title")}>
      <Toolbar />
      <NoticeBar />
      <SystemUpdates />
      <Ribbon />
      <SidebarLayout>
        <Pane w={0} />
        <Pane w={1} />
      </SidebarLayout>
      {terminal && <TerminalPanel api={api} cwd={terminalCwd} />}
      <JobsPanel />
      <FKeys />
      <Toasts />
      <DragHint />
      <ContextMenu />
      <Dialogs api={api} />
    </main>
  );
}

export interface AppProps {
  /** Schnittstelle zum Kern: in der App die echte, im Entwurf und in Tests `createMockApi()`. */
  api: DualfileApi;
  options?: ControllerOptions;
  /** Fertig gebauter Speicher samt Controller (Tests brauchen Zugriff darauf). */
  runtime?: AppRuntime;
}

export function App({ api, options, runtime }: AppProps) {
  const built = useMemo(() => runtime ?? createRuntime(api, options), [api, options, runtime]);
  return (
    <RuntimeProvider runtime={built}>
      <Shell api={api} />
      <SshPromptDialogs api={api} />
    </RuntimeProvider>
  );
}
