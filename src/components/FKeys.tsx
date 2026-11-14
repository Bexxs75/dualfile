import { useAppState, useController, useT } from "../app/context";
import { activeTabOf, otherWindow } from "../app/state";
import { isArchivePath } from "../state/paths";
import type { TranslationKey } from "../i18n";
import type { Command } from "../state/keymap";

const KEYS: { label: string; text: TranslationKey; command: Command; danger?: boolean }[] = [
  { label: "F2", text: "fkey.f2", command: "rename" },
  { label: "F3", text: "fkey.f3", command: "view" },
  { label: "F4", text: "fkey.f4", command: "edit" },
  { label: "F5", text: "fkey.f5", command: "copy" },
  { label: "F6", text: "fkey.f6", command: "move" },
  { label: "F7", text: "fkey.f7", command: "makeDir" },
  { label: "F8", text: "fkey.f8", command: "trash", danger: true },
  { label: "⇧F8", text: "fkey.shiftF8", command: "deletePermanent", danger: true },
];

/** Schreibende Befehle: in einem Archivfenster ausgegraut. */
const WRITES = new Set<Command>(["edit", "rename", "move", "makeDir", "trash", "deletePermanent"]);

/** F-Tasten-Leiste: dieselben Befehle wie die Tastatur, klickbar. */
export function FKeys() {
  const controller = useController();
  const tr = useT();
  const readOnly = useAppState((state) => isArchivePath(activeTabOf(state, state.active).path));
  const targetReadOnly = useAppState((state) => isArchivePath(activeTabOf(state, otherWindow(state.active)).path));
  return (
    <div className="fkeys" role="toolbar" aria-label={tr("ui.fkeys")}>
      {KEYS.map((key) => {
        const blocked = (readOnly && WRITES.has(key.command)) || (targetReadOnly && key.command === "copy");
        return (
        <button
          key={key.label}
          type="button"
          className={key.danger ? "danger" : undefined}
          disabled={blocked}
          title={blocked ? tr("error.readOnly") : undefined}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => controller.command(key.command)}
        >
          <kbd>{key.label}</kbd>{" "}<span>{tr(key.text)}</span>
        </button>
        );
      })}
    </div>
  );
}
