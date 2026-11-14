import type { UndoEntry } from "../ipc/types";
import type { createTranslator } from "../i18n";

export function undoDescription(entry: UndoEntry, tr: ReturnType<typeof createTranslator>): string {
  const kind = tr(`undoLabel.${entry.kind}`);
  return entry.first ? `${kind}: ${entry.first}${entry.more ? tr("undo.more", { count: entry.more }) : ""}` : kind;
}

export function undoShortcuts(platform: string): { undo: string; history: string } {
  return /Mac|iPhone|iPad/i.test(platform)
    ? { undo: "Cmd+Z", history: "Cmd+Shift+Z" }
    : { undo: "Ctrl+Z", history: "Ctrl+Shift+Z" };
}
