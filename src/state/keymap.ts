export type BookmarkCommand = "bookmark1" | "bookmark2" | "bookmark3" | "bookmark4" | "bookmark5" | "bookmark6" | "bookmark7" | "bookmark8" | "bookmark9";

export type Command =
  | "switchPane"
  | "moveUp"
  | "moveDown"
  | "home"
  | "end"
  | "pageUp"
  | "pageDown"
  | "extendUp"
  | "extendDown"
  | "extendHome"
  | "extendEnd"
  | "extendPageUp"
  | "extendPageDown"
  | "open"
  | "goUp"
  | "goBack"
  | "goForward"
  | "rename"
  | "multiRename"
  | "pack"
  | "compareDirs"
  | "compareFiles"
  | "view"
  | "quickView"
  | "edit"
  | "copy"
  | "move"
  | "makeDir"
  | "trash"
  | "deletePermanent"
  | "toggleSelection"
  | "selectByPattern"
  | "deselectByPattern"
  | "invertSelection"
  | "selectAll"
  | "pathInput"
  | "refresh"
  | "toggleHidden"
  | "swapPanes"
  | "newTab"
  | "closeTab"
  | "nextTab"
  | "prevTab"
  | "undo"
  | "escape"
  | BookmarkCommand;

export interface KeyboardEventLike {
  key: string;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
}

/** Wo der Fokus liegt: Dateiliste, Texteingabe (Pfadzeile, Filter, Dialogfeld) oder Dialog. */
export type KeyContext = "list" | "input" | "dialog";

/** "bookmark3" → 3 */
export const bookmarkNumber = (command: BookmarkCommand): number => Number(command.slice(8));

const ARROWS: Record<string, [Command, Command]> = {
  ArrowUp: ["moveUp", "extendUp"],
  ArrowDown: ["moveDown", "extendDown"],
  Home: ["home", "extendHome"],
  End: ["end", "extendEnd"],
  PageUp: ["pageUp", "extendPageUp"],
  PageDown: ["pageDown", "extendPageDown"],
};

const CTRL_LETTERS: Record<string, Command> = {
  a: "selectAll",
  l: "pathInput",
  r: "refresh",
  h: "toggleHidden",
  u: "swapPanes",
  t: "newTab",
  w: "closeTab",
  z: "undo",
  m: "multiRename",
  q: "quickView",
};

// Tab-Befehle sind in Eingabefeldern unbedenklich: Strg+T/W/Tab hat dort keine Textbedeutung
// (anders als Strg+A/Z, die im Feld Text markieren bzw. zurücknehmen und deshalb dem Feld gehören).
const TAB_COMMANDS = new Set<Command>(["newTab", "closeTab", "nextTab", "prevTab"]);

function listCommand(event: KeyboardEventLike): Command | null {
  const { key, ctrlKey = false, shiftKey = false, altKey = false, metaKey = false } = event;
  // Meta und nicht belegte Alt-Kombinationen gehören dem System (z. B. Alt+F4), nie uns.
  if (metaKey) return null;
  if (altKey && !ctrlKey && !shiftKey && key === "ArrowLeft") return "goBack";
  if (altKey && !ctrlKey && !shiftKey && key === "ArrowRight") return "goForward";
  if (altKey && !ctrlKey && !shiftKey && key === "ArrowUp") return "goUp";
  if (altKey) return !ctrlKey && !shiftKey && /^[1-9]$/.test(key) ? (`bookmark${key}` as BookmarkCommand) : null;
  if (ctrlKey) {
    if (key === "Tab") return shiftKey ? "prevTab" : "nextTab";
    if (shiftKey) {
      const letter = key.toLowerCase();
      if (letter === "p") return "pack";
      if (letter === "d") return "compareDirs";
      if (letter === "f") return "compareFiles";
      return null;
    }
    return CTRL_LETTERS[key.toLowerCase()] ?? null;
  }
  const arrow = ARROWS[key];
  if (arrow) return arrow[shiftKey ? 1 : 0];
  if (shiftKey) {
    // Umschalt-Varianten: bewusst nur diese; Umschalt+F5/Enter usw. bleiben unbelegt.
    if (key === "F6") return "rename";
    if (key === "F8" || key === "Delete") return "deletePermanent";
    if (key === "Tab") return "switchPane";
    // "+" und "*" brauchen auf vielen Layouts Umschalt.
    if (key !== "+" && key !== "*") return null;
  }
  switch (key) {
    case "Tab":
      return "switchPane";
    case "Enter":
      return "open";
    case "Backspace":
      return "goUp";
    case "F2":
      return "rename";
    case "F3":
      return "view";
    case "F4":
      return "edit";
    case "F5":
      return "copy";
    case "F6":
      return "move";
    case "F7":
      return "makeDir";
    case "F8":
    case "Delete":
      return "trash"; // Papierkorb ist der Standard; endgültig nur mit Umschalt und Dialog
    case "Insert":
    case " ":
      return "toggleSelection";
    case "+":
      return "selectByPattern";
    case "-":
      return "deselectByPattern";
    case "*":
      return "invertSelection";
    case "Escape":
      return "escape";
    default:
      return null;
  }
}

/**
 * Reine Zuordnung Taste → Befehl. In Eingabefeldern und Dialogen greifen die Listenbefehle nicht
 * (Leertaste, Pfeile, Rücktaste, +/-/* gehören dem Feld). Es bleibt Esc (Eingabe/Dialog abbrechen)
 * und im Feld zusätzlich die Tab-Befehle. Enter im Dialog löst der fokussierte Knopf selbst aus.
 */
export function mapKey(event: KeyboardEventLike, context: KeyContext = "list"): Command | null {
  const command = listCommand(event);
  if (context === "list") return command;
  if (command === "escape") return command;
  if (context === "input" && command !== null && TAB_COMMANDS.has(command)) return command;
  return null;
}

export type DiffKeyCommand = "nextDifference" | "prevDifference";

/**
 * Tasten der Textvergleich-Ansicht (eigener Dialog, deshalb getrennt von `mapKey`): n / F3 zum nächsten,
 * p / Umschalt+F3 zum vorigen Unterschied. Strg/Alt/Meta-Kombinationen bleiben frei.
 */
export function mapDiffKey(event: KeyboardEventLike): DiffKeyCommand | null {
  const { key, ctrlKey = false, shiftKey = false, altKey = false, metaKey = false } = event;
  if (ctrlKey || altKey || metaKey) return null;
  if (key === "F3") return shiftKey ? "prevDifference" : "nextDifference";
  if (shiftKey) return null;
  if (key === "n") return "nextDifference";
  if (key === "p") return "prevDifference";
  return null;
}
