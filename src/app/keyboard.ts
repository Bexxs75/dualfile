import { useEffect } from "react";
import { mapKey, type KeyContext, type Command } from "../state/keymap";
import type { Controller } from "./controller";
import type { AppStore } from "./controller";
import { activeTabOf } from "./state";
import { isModalOpen } from "../components/Modal";

type GlobalAction = "palette" | "connections" | "settings" | "filter" | "pin" | "undoHistory";
export const SIDEBAR_COMMANDS = new Set<Command | GlobalAction>(["palette", "connections", "settings", "filter", "pin", "undo", "undoHistory", "toggleSidebar", "toggleSidebarIcons", "focusSidebar", "switchPane", "newTab", "closeTab", "nextTab", "prevTab", "pathInput", "refresh", "toggleHidden", "swapPanes", "terminal", "quickView", "bookmark1", "bookmark2", "bookmark3", "bookmark4", "bookmark5", "bookmark6", "bookmark7", "bookmark8", "bookmark9"]);
const CTRL_ACTIONS: Partial<Record<string, GlobalAction | "undo">> = { k: "palette", p: "palette", n: "connections", ",": "settings", f: "filter", d: "pin", z: "undo" };
const INPUT_TAGS = new Set(["INPUT", "TEXTAREA", "SELECT"]);

/**
 * Globaler Tastaturhandler. Zuordnung über `mapKey` (Kontext Liste/Eingabe); solange ein Dialog,
 * eine Rückfrage oder das Kontextmenü offen ist, gehört die Tastatur dem Dialog. Dazu kommen
 * wenige Kürzel außerhalb der Tabelle: Strg+, (Einstellungen), Strg+D (anheften), Strg+F (Filter),
 * Umschalt+F10 und Menütaste (Kontextmenü).
 */
export function useGlobalKeys(store: AppStore, controller: Controller): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (event.defaultPrevented || event.isComposing) return;
      const state = store.getState();
      if (state.phase !== "ready") return;
      if (isModalOpen()) return;
      if (state.closeRequested !== null || state.itemErrors.length > 0 || state.dialog !== null || state.conflicts.length > 0 || state.menu !== null || state.lister !== null || state.compare !== null || state.diff !== null) return;
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (target?.closest('[role="dialog"], [role="alertdialog"], [role="menu"]')) return;
      if (target?.closest(".terminal-panel")) return;
      if (target?.closest(".sidebar")) {
        const command = mapKey(event, "list", state.settings.keybindings);
        const plainCtrl = event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey;
        const global: GlobalAction | "undo" | undefined = plainCtrl
          ? CTRL_ACTIONS[event.key.toLowerCase()]
          : !event.altKey && event.ctrlKey !== event.metaKey && event.key.toLowerCase() === "z" ? (event.shiftKey ? "undoHistory" : "undo") : undefined;
        const action = global ?? command;
        if (!action || !SIDEBAR_COMMANDS.has(action)) return;
        if (!global && command) { event.preventDefault(); controller.command(command); if (command === "switchPane") controller.focusFileList(); return; }
      }
      const typing = target !== null && (INPUT_TAGS.has(target.tagName) || target.isContentEditable);
      const context: KeyContext = typing ? "input" : "list";
      // Ein fokussierter Knopf löst sich mit Enter/Leertaste selbst aus.
      if (!typing && (event.key === "Enter" || event.key === " ") && target?.closest("button")) return;

      if (!typing && !event.altKey && (event.ctrlKey !== event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        if (!event.repeat) {
          if (event.shiftKey) controller.openDialog({ kind: "history" });
          else void controller.undo();
        }
        return;
      }
      const mod = event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey;
      if (mod && ["k", "p"].includes(event.key.toLowerCase())) {
        event.preventDefault();
        if (!event.repeat) document.dispatchEvent(new CustomEvent("dualfile:command-palette"));
        return;
      }
      if (mod && event.key.toLowerCase() === "n" && !typing) {
        event.preventDefault();
        controller.openDialog({ kind: "connections" });
        return;
      }
      if (mod && event.key === ",") {
        event.preventDefault();
        controller.openSettings();
        return;
      }
      if (mod && event.key.toLowerCase() === "d" && !typing) {
        event.preventDefault();
        controller.togglePin();
        return;
      }
      if ((mod && event.key.toLowerCase() === "f") || (!typing && !event.ctrlKey && !event.altKey && !event.metaKey && event.key === "/")) {
        event.preventDefault();
        document.dispatchEvent(new CustomEvent("dualfile:focus-filter", { detail: state.active }));
        return;
      }
      if (!typing && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) {
        event.preventDefault();
        const row = document.querySelector<HTMLElement>(`[data-pane="${state.active}"] .row.cur`);
        const box = row?.getBoundingClientRect();
        controller.openContextMenu(state.active, row ? activeTabOf(state, state.active).selection.focus : null, box ? box.left + 24 : 80, box ? box.bottom : 120);
        return;
      }

      // Outside the file lists, Tab follows the native order (toolbar and toast buttons).
      if (event.key === "Tab" && !event.ctrlKey && !event.altKey && !event.metaKey && target?.closest("button")) return;
      const command = mapKey(event, context, state.settings.keybindings);
      if (command) {
        event.preventDefault();
        controller.command(command);
        return;
      }
      if (context === "list" && controller.isTypeaheadKey(event)) {
        event.preventDefault();
        controller.typeahead(event.key);
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [store, controller]);
}

