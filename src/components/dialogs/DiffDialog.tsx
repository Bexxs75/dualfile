import { useEffect, useMemo, useRef, useState } from "react";
import { useController, useT } from "../../app/context";
import type { DiffHunkKind } from "../../ipc/types";
import { mapDiffKey } from "../../state/keymap";
import { buildDiffRows, differenceHunks, type DiffRow, type DiffSession } from "../../state/compare";
import { displayPath } from "../../state/paths";
import { Modal } from "../Modal";
import { useDialogFocus } from "./useDialogFocus";

const MARK: Record<DiffHunkKind, string> = { equal: "", insert: "+", delete: "−", replace: "~" };
/** Gleiche Abschnitte ab dieser Länge werden zusammengeklappt: oben und unten bleibt Kontext. */
const COLLAPSE_OVER = 12;
const CONTEXT = 3;
const COLLAPSE_DEFAULT_ROWS = 2000;

type ViewItem = { row: DiffRow } | { hidden: number; key: string };

function viewOf(rows: readonly DiffRow[], collapsed: boolean): ViewItem[] {
  if (!collapsed) return rows.map((row) => ({ row }));
  const out: ViewItem[] = [];
  for (let i = 0; i < rows.length; ) {
    const row = rows[i]!;
    let end = i;
    while (end < rows.length && rows[end]!.hunk === row.hunk) end += 1;
    const length = end - i;
    if (row.kind === "equal" && length > COLLAPSE_OVER) {
      for (let k = i; k < i + CONTEXT; k += 1) out.push({ row: rows[k]! });
      out.push({ hidden: length - 2 * CONTEXT, key: `h${row.hunk}` });
      for (let k = end - CONTEXT; k < end; k += 1) out.push({ row: rows[k]! });
    } else for (let k = i; k < end; k += 1) out.push({ row: rows[k]! });
    i = end;
  }
  return out;
}

/**
 * Textvergleich zweier Dateien: zwei Spalten aus den Abschnitten des Kerns, Marken (Text und Symbol)
 * für eingefügte, gelöschte und geänderte Zeilen, Springen mit n/p oder F3/Umschalt+F3. Zeilen sind
 * nur Text (nie HTML) und brechen um.
 */
export function DiffDialog({ session }: { session: DiffSession }) {
  const controller = useController();
  const tr = useT();
  const anchor = useRef<HTMLDivElement>(null);
  const currentRow = useRef<HTMLTableRowElement>(null);
  const result = session.result;
  const rows = useMemo(() => (result ? buildDiffRows(result.hunks) : []), [result]);
  const differences = useMemo(() => (result ? differenceHunks(result.hunks) : []), [result]);
  const [collapsedChoice, setCollapsedChoice] = useState<boolean | null>(null);
  const collapsed = collapsedChoice ?? rows.length > COLLAPSE_DEFAULT_ROWS;
  const view = useMemo(() => viewOf(rows, collapsed), [rows, collapsed]);
  const currentHunk = session.current >= 0 ? differences[session.current] : undefined;
  useDialogFocus(anchor, session.phase);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (event.defaultPrevented) return;
      const command = mapDiffKey(event);
      if (!command) return;
      event.preventDefault();
      controller.diffStep(command === "nextDifference" ? 1 : -1);
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [controller]);

  useEffect(() => {
    currentRow.current?.scrollIntoView?.({ block: "center" });
  }, [session.current, view]);

  let marked = false;
  return (
    <Modal
      title={tr("diff.title")}
      className="cmp diff"
      onClose={() => controller.closeDiff()}
      footer={
        <>
          <span className="hint">{tr("diff.hint")}</span>
          <span className="sp" />
          <button type="button" className="tb primary" data-autofocus onClick={() => controller.closeDiff()}>
            {tr("action.close")}
          </button>
        </>
      }
    >
      <div ref={anchor} className="diff-body">
        <div className="cmp-roots">
          <span>{tr("diff.left", { path: displayPath(session.left) })}</span>
          <span>{tr("diff.right", { path: displayPath(session.right) })}</span>
        </div>
        <div className="cmp-opts">
          <label className="check">
            <input
              type="checkbox"
              checked={session.options.ignoreWhitespace}
              onChange={(event) => controller.setDiffOption({ ignoreWhitespace: event.target.checked })}
            />
            {tr("diff.ignoreWhitespace")}
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={session.options.ignoreCase}
              onChange={(event) => controller.setDiffOption({ ignoreCase: event.target.checked })}
            />
            {tr("diff.ignoreCase")}
          </label>
          <span className="sp" />
          <button type="button" className="tb small" disabled={!result || session.current <= 0} onClick={() => controller.diffStep(-1)}>
            {tr("diff.prev")}
          </button>
          <button
            type="button"
            className="tb small"
            disabled={!result || session.current < 0 || session.current >= differences.length - 1}
            onClick={() => controller.diffStep(1)}
          >
            {tr("diff.next")}
          </button>
          {result && differences.length > 0 ? (
            <span role="status">{tr("diff.position", { current: session.current + 1, total: differences.length })}</span>
          ) : null}
        </div>
        {session.phase === "loading" ? <p role="status">{tr("diff.loading")}</p> : null}
        {session.phase === "error" ? (
          <p className="field-error" role="alert">
            {session.error}
          </p>
        ) : null}
        {result ? (
          <>
            <p className="muted" role="status">
              {differences.length === 0 ? `${tr("diff.none")} ` : ""}
              {tr("diff.summary", { left: result.leftLines, right: result.rightLines })}
              {rows.length > COLLAPSE_OVER ? (
                <>
                  {" "}
                  <button type="button" className="tb small" onClick={() => setCollapsedChoice(!collapsed)}>
                    {collapsed ? tr("diff.expand") : tr("diff.collapse")}
                  </button>
                </>
              ) : null}
            </p>
            <div className="cmp-scroll diff-scroll">
              <table className="tbl diff-table" aria-label={tr("diff.table")}>
                <thead>
                  <tr>
                    <th scope="col">{tr("diff.col.mark")}</th>
                    <th scope="col">{tr("diff.col.leftNo")}</th>
                    <th scope="col">{tr("diff.col.left")}</th>
                    <th scope="col">{tr("diff.col.rightNo")}</th>
                    <th scope="col">{tr("diff.col.right")}</th>
                  </tr>
                </thead>
                <tbody>
                  {view.map((item, index) => {
                    if ("hidden" in item) {
                      return (
                        <tr key={item.key} className="skip">
                          <td colSpan={5}>{tr("diff.hidden", { count: item.hidden })}</td>
                        </tr>
                      );
                    }
                    const { row } = item;
                    const isCurrent = row.hunk === currentHunk;
                    const first = isCurrent && !marked;
                    if (first) marked = true;
                    return (
                      <tr
                        key={index}
                        ref={first ? currentRow : undefined}
                        className={`dr ${row.kind}${isCurrent ? " cur" : ""}`}
                        data-kind={row.kind}
                        aria-current={first ? "true" : undefined}
                      >
                        <td className="mk">
                          <span aria-hidden="true">{MARK[row.kind]}</span>
                          {row.kind !== "equal" ? <span className="sr">{tr(`diff.mark.${row.kind}`)}</span> : null}
                        </td>
                        <td className="no">{row.leftNo ?? ""}</td>
                        <td className={row.left === null ? "tx gap" : "tx"}>{row.left ?? ""}</td>
                        <td className="no">{row.rightNo ?? ""}</td>
                        <td className={row.right === null ? "tx gap" : "tx"}>{row.right ?? ""}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </>
        ) : null}
      </div>
    </Modal>
  );
}
