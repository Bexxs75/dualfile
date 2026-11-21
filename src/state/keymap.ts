export type BookmarkCommand = "bookmark1" | "bookmark2" | "bookmark3" | "bookmark4" | "bookmark5" | "bookmark6" | "bookmark7" | "bookmark8" | "bookmark9";

export type Command =
  | "toggleSidebar" | "toggleSidebarIcons" | "focusSidebar"
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
  | "terminal"
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
  /** Physische Taste (layoutunabhängig), z.B. "Backquote". Für Tasten, die auf manchen Layouts
   *  (z.B. deutsch) als Tot-/Akzenttaste nicht direkt tippbar sind. */
  code?: string;
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

// Tab-Befehle sind in Eingabefeldern unbedenklich: Strg+T/W/Tab hat dort keine Textbedeutung
// (anders als Strg+A/Z, die im Feld Text markieren bzw. zurücknehmen und deshalb dem Feld gehören).
const TAB_COMMANDS = new Set<Command>(["newTab", "closeTab", "nextTab", "prevTab"]);

/**
 * Umbelegbare Befehle mit ihrer Standardtaste (Spec-Format siehe `eventToSpec`). Alles andere
 * (Pfeile, Leer, Enter, Rücktaste, Tab, Alt-Lesezeichen, Esc, +/-/*) bleibt strukturell fest.
 */
export const REBINDABLE: Record<RebindableCommand, string> = {
  toggleSidebar: "F9", toggleSidebarIcons: "Shift+F9", focusSidebar: "",
  rename: "F2",
  view: "F3",
  edit: "F4",
  copy: "F5",
  move: "F6",
  makeDir: "F7",
  trash: "F8",
  deletePermanent: "Shift+F8",
  selectAll: "Ctrl+A",
  pathInput: "Ctrl+L",
  refresh: "Ctrl+R",
  toggleHidden: "Ctrl+H",
  swapPanes: "Ctrl+U",
  newTab: "Ctrl+T",
  closeTab: "Ctrl+W",
  undo: "Ctrl+Z",
  multiRename: "Ctrl+M",
  quickView: "Ctrl+Q",
  terminal: "Ctrl+`",
  pack: "Ctrl+Shift+P",
  compareDirs: "Ctrl+Shift+D",
  compareFiles: "Ctrl+Shift+F",
};

export type RebindableCommand =
  | "toggleSidebar" | "toggleSidebarIcons" | "focusSidebar"
  | "rename" | "view" | "edit" | "copy" | "move" | "makeDir" | "trash" | "deletePermanent"
  | "selectAll" | "pathInput" | "refresh" | "toggleHidden" | "swapPanes" | "newTab" | "closeTab"
  | "undo" | "multiRename" | "terminal" | "quickView" | "pack" | "compareDirs" | "compareFiles";

export const REBINDABLE_COMMANDS = Object.keys(REBINDABLE) as RebindableCommand[];

const DEFAULT_SPECS = new Set<string>(Object.values(REBINDABLE));

/**
 * Tasten, die der globale Handler vor `mapKey` selbst abfängt (Palette, Verbindungen, Anheften,
 * Filter, Rückgängig). Sie lassen sich nicht auf andere Befehle legen.
 */
export const RESERVED_SPECS: ReadonlySet<string> = new Set(["Ctrl+K", "Ctrl+P", "Ctrl+N", "Ctrl+D", "Ctrl+F", "Ctrl+Z"]);

/**
 * Kanonischer Spec der umbelegbaren Tastenform (`F3`, `Shift+F6`, `Ctrl+A`, `Ctrl+Shift+F`) oder `null`
 * für alles andere (Pfeile, Leer, Enter, Entf, Tab, Alt/Meta-Kombinationen, Buchstaben ohne Strg).
 * Strg-Kombinationen nur mit Buchstaben/Ziffern; Satzzeichen (Strg+, u. a.) bleiben unbelegbar.
 */
export function eventToSpec(event: KeyboardEventLike): string | null {
  const { key, code, ctrlKey = false, shiftKey = false, altKey = false, metaKey = false } = event;
  if (altKey || metaKey) return null;
  if (/^F([1-9]|1[0-2])$/.test(key)) return ctrlKey ? null : shiftKey ? `Shift+${key}` : key;
  // Backtick is a dead/accent key on several layouts (e.g. German), so match the physical key too.
  if (ctrlKey && !shiftKey && (key === "`" || code === "Backquote")) return "Ctrl+`";
  if (ctrlKey && /^[a-z0-9]$/i.test(key)) return `Ctrl+${shiftKey ? "Shift+" : ""}${key.toUpperCase()}`;
  return null;
}

/** Strukturelle Tasten (nicht umbelegbar) sowie die festen Aliase Entf/Umschalt+Entf/Umschalt+F6. */
function structuralCommand(event: KeyboardEventLike): Command | null {
  const { key, ctrlKey = false, shiftKey = false, altKey = false, metaKey = false } = event;
  // Meta und nicht belegte Alt-Kombinationen gehören dem System (z. B. Alt+F4), nie uns.
  if (metaKey) return null;
  if (altKey && !ctrlKey && !shiftKey && key === "ArrowLeft") return "goBack";
  if (altKey && !ctrlKey && !shiftKey && key === "ArrowRight") return "goForward";
  if (altKey && !ctrlKey && !shiftKey && key === "ArrowUp") return "goUp";
  if (altKey) return !ctrlKey && !shiftKey && /^[1-9]$/.test(key) ? (`bookmark${key}` as BookmarkCommand) : null;
  if (ctrlKey) {
    if (key === "Tab") return shiftKey ? "prevTab" : "nextTab";
    return null;
  }
  const arrow = ARROWS[key];
  if (arrow) return arrow[shiftKey ? 1 : 0];
  if (shiftKey) {
    // Umschalt-Varianten: bewusst nur diese; Umschalt+F5/Enter usw. bleiben unbelegt.
    if (key === "F6") return "rename";
    if (key === "Delete") return "deletePermanent";
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

function resolveCommand(event: KeyboardEventLike, bindings?: Record<string, string>): Command | null {
  const effective = effectiveBindings(bindings);
  const reverse = new Map<string, Command>();
  for (const [command, spec] of Object.entries(effective)) { if (spec) reverse.set(spec, command as Command); }
  const spec = eventToSpec(event);
  // Umbelegbare Tasten werden ausschließlich hier aufgelöst: eine verschobene, von niemandem
  // übernommene Standardtaste ergibt null und fällt nicht an die strukturelle Logik.
  if (spec && (reverse.has(spec) || DEFAULT_SPECS.has(spec))) return reverse.get(spec) ?? null;
  return structuralCommand(event);
}

/**
 * Reine Zuordnung Taste → Befehl. In Eingabefeldern und Dialogen greifen die Listenbefehle nicht
 * (Leertaste, Pfeile, Rücktaste, +/-/* gehören dem Feld). Es bleibt Esc (Eingabe/Dialog abbrechen)
 * und im Feld zusätzlich die Tab-Befehle. Enter im Dialog löst der fokussierte Knopf selbst aus.
 * `bindings` sind die Nutzer-Overrides (Befehl → Spec); ohne sie gelten die Standardtasten.
 */
export function mapKey(event: KeyboardEventLike, context: KeyContext = "list", bindings?: Record<string, string>): Command | null {
  const command = resolveCommand(event, bindings);
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

/** Existing explicit bindings win over newly introduced defaults, including in settings UI. */
export function effectiveBindings(bindings?: Record<string, string>): Record<RebindableCommand, string> {
  const effective = { ...REBINDABLE };
  for (const command of REBINDABLE_COMMANDS) if (typeof bindings?.[command] === "string") effective[command] = bindings[command]!;
  for (const command of ["toggleSidebar", "toggleSidebarIcons"] as const) {
    if (bindings?.[command] === undefined && Object.entries(bindings ?? {}).some(([other, key]) => other !== command && key === REBINDABLE[command])) effective[command] = "";
  }
  return effective;
}

/** Menu legends share the effective bindings; fixed aliases live beside the key resolver. */
export function menuShortcut(command: Command | "pin", bindings?: Record<string, string>, language = "en"): string {
  const fixed: Partial<Record<Command | "pin", string>> = { goBack: "Alt+←", goForward: "Alt+→", open: "Enter", pin: "Ctrl+D", deletePermanent: "Shift+Delete" };
  const spec = fixed[command] ?? effectiveBindings(bindings)[command as RebindableCommand] ?? "";
  return language === "de" ? spec.replace("Ctrl", "Strg").replace("Shift", "Umsch").replace("Delete", "Entf") : spec;
}
