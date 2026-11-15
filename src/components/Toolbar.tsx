import { createCommands } from "../app/commands";
import { CommandPalette } from "./CommandPalette";
import { useEffect, useState } from "react";
import { SimilarDialog } from "./dialogs/SimilarDialog";
import { UsageDialog, DupesDialog } from "./dialogs/SpaceDialogs";
import { useAppState, useController, useT } from "../app/context";
import { activeTabOf } from "../app/state";
import { parseRemotePath } from "../state/paths";
import { Icon, type IconName } from "./Icon";
import { TerminalShortcutHint } from "./ShortcutHint";
import { HeaderMenu, type HeaderMenuItem } from "./HeaderMenu";

export function Toolbar() {
  const [space, setSpace] = useState<{ kind: "usage" | "dupes" | "similar"; root: string } | null>(null);
  const state = useAppState(state => state);
  const [palette, setPalette] = useState(false);
  const [recent, setRecent] = useState<string[]>([]);
  const path = useAppState(state => activeTabOf(state, state.active).path);
  const connections = useAppState(state => state.connections);
  const remoteConnections = useAppState(state => state.remoteConnections);
  const showHidden = useAppState(state => state.showHidden);
  const terminal = useAppState(state => state.terminal);
  const quickView = useAppState(state => state.quickView);
  const mirror = useAppState(state => state.settings.mirror);
  const controller = useController();
  const tr = useT();
  const remote = parseRemotePath(path);
  const current = remote ? remoteConnections[remote.id] : undefined;
  const connection = current ?? connections.find(e => e.connected) ?? connections[0];
  const connectionLabel = connection ? `${connection.profile.name} · ${connection.profile.protocol.toUpperCase()}` : tr("ssh.connections");
  const connectionState = connection ? tr(connection.connected ? "header.connected" : "ssh.disconnected") : "";
  const entry = (action: "new" | "import" | "edit" | "connect", profile?: typeof connections[number]["profile"]) => controller.openDialog({ kind: "connections", entry: { action, profile } });
  const connectionItems: HeaderMenuItem[] = connections.flatMap(({ profile, connected }) => [
    { kind: "heading", label: `${profile.name} · ${profile.protocol.toUpperCase()} · ${tr(connected ? "header.connected" : "ssh.disconnected")}`, connected },
    { label: tr(connected ? "ssh.disconnect" : "header.reconnect"), ariaLabel: `${profile.name} — ${tr(connected ? "ssh.disconnect" : "header.reconnect")}`, icon: connected ? "disconnect" : "connect", action: () => { if (connected) void controller.disconnectConnection(profile.id); else entry("connect", profile); } },
    { label: `${tr("ssh.edit")} …`, ariaLabel: `${profile.name} — ${tr("ssh.edit")} …`, icon: "edit", action: () => entry("edit", profile) },
  ]);
  connectionItems.push(
    { label: tr("ssh.new"), icon: "plus", action: () => entry("new") },
    { label: tr("ssh.import"), icon: "import", action: () => entry("import") },
    { label: tr("header.manageConnections"), icon: "list", action: () => controller.openDialog({ kind: "connections" }) },
  );
  const commands = createCommands(state, controller, tr, kind => setSpace({ kind, root: path }));
  const tools: HeaderMenuItem[] = ["multiRename", "compareDirs", "usage", "dupes", "similar", "history"].map(id => {
    const command = commands.find(c => c.id === id)!;
    return { label: command.title, icon: command.icon, action: () => { if (command.enabled(state)) command.execute(); } };
  });
  const openPalette = () => {
    document.querySelector<HTMLElement>(`[data-pane="${state.active}"] [role="grid"]`)?.focus();
    setPalette(true);
  };
  useEffect(() => {
    document.addEventListener("dualfile:command-palette", openPalette);
    return () => document.removeEventListener("dualfile:command-palette", openPalette);
  }, [state.active]);
  const iconButton = (icon: IconName, label: string, title: string, action: () => void, pressed?: boolean) => <button type="button" className="header-icon" onMouseDown={e => e.preventDefault()} aria-label={label} title={title} aria-pressed={pressed} onClick={action}><Icon name={icon} /></button>;
  return <>
    <div className="bar header-toolbar bg-surface border-line" role="toolbar" aria-label={tr("ui.toolbar")}>
      <span className="brand text-muted"><Icon name="columns" />{tr("app.title")}</span>
      {iconButton("hidden", tr("menu.view.hidden"), tr("tip.hidden"), () => controller.command("toggleHidden"), showHidden)}
      {iconButton("swap", tr("menu.view.swapPanes"), tr("tip.swapPanes"), () => controller.command("swapPanes"))}
      {iconButton("preview", tr("menu.view.quickView"), tr("tip.quickView"), () => controller.command("quickView"), quickView)}
      <TerminalShortcutHint>{iconButton("terminal", tr("menu.view.terminal"), tr("tip.terminal"), () => controller.command("terminal"), terminal)}</TerminalShortcutHint>
      <HeaderMenu label={tr("header.tools")} items={tools}><Icon name="tools" />{tr("header.tools")}</HeaderMenu>
      <button type="button" className="command-placeholder" onClick={openPalette} aria-haspopup="dialog" aria-expanded={palette}><Icon name="search" /><span>{tr("header.command")}</span><kbd>{tr("header.commandKey")}</kbd></button>
      <span className="sp" />
      <HeaderMenu className="connections-menu" label={`${connectionLabel}${connectionState ? ` · ${connectionState}` : ""}`} title={`${connectionLabel}${connectionState ? ` · ${connectionState}` : ""} (Ctrl+N)`} items={connectionItems}>
        {connection && <span className={`connection-dot ${connection.connected ? "connected" : "disconnected"}`} />}<Icon name="connect" /><span className="connection-name">{connectionLabel}</span>
      </HeaderMenu>
      <button type="button" className="header-chip" onMouseDown={e => e.preventDefault()} aria-label={tr("menu.view.mirror")} aria-pressed={mirror} title={tr("tip.mirror")} onClick={() => controller.setMirror(!mirror)}><Icon name="couple" /><span>{tr("header.couple")}</span><span className="text-muted">{tr(mirror ? "header.on" : "header.off")}</span></button>
      {iconButton("settings", tr("menu.view.settings"), `${tr("tip.settings")} (Ctrl+,)`, () => controller.openDialog({ kind: "settings" }))}
    </div>
    {palette && <CommandPalette commands={commands} recent={recent} onRun={id => setRecent(previous => [id, ...previous.filter(item => item !== id)].slice(0, 5))} onClose={() => setPalette(false)} />}
    {space?.kind === "usage" && <UsageDialog root={space.root} onClose={() => setSpace(null)} />}
    {space?.kind === "similar" && <SimilarDialog root={space.root} onClose={() => setSpace(null)} />}
    {space?.kind === "dupes" && <DupesDialog root={space.root} onClose={() => setSpace(null)} />}
  </>;
}
