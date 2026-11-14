import { useAppState, useController, useT } from "../app/context";

/** Hinweis nach dem Laden der Einstellungen, wenn etwas repariert, gesichert oder nicht überschrieben wird. */
export function NoticeBar() {
  const outcome = useAppState((state) => state.outcome);
  const visible = useAppState((state) => state.noticeVisible);
  const controller = useController();
  const tr = useT();
  if (!visible) return null;
  let text: string | null = null;
  switch (outcome.kind) {
    case "repaired":
      text = tr("notice.repaired", { messages: outcome.messages.join("; ") });
      break;
    case "recoveredFromCorrupt":
      text = tr("notice.recoveredFromCorrupt");
      break;
    case "newerVersion":
      text = tr("notice.newerVersion", { version: outcome.version });
      break;
    case "migrated":
      text = tr("notice.migrated", { from: outcome.from, to: outcome.to });
      break;
    default:
      return null;
  }
  return (
    <div className="notice" role="status" aria-label={tr("ui.notice")}>
      <span>{text}</span>
      <button type="button" className="tb small" onClick={() => controller.dismissNotice()}>
        {tr("action.close")}
      </button>
    </div>
  );
}
