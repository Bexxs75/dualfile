import type { CSSProperties } from "react";
import { useAppState, useT } from "../app/context";
import { Icon } from "../components/Icon";
import { themeVariables, effectiveAccent } from "../themes/tokens";
import { paneTintVariables } from "../themes/paneTint";
import { columnTemplate, columnWidthsFor, ownerColumnTemplates } from "../state/columns";

/** Uses the list's real grid, row, current-selection and tint styles, without interactive elements. */
export function SettingsPreview() {
  const settings = useAppState(s => s.settings), tr = useT();
  const ext = !!settings.extensionColumn;
  const owners = [!!settings.showPermissionsColumn, !!settings.showUidColumn, !!settings.showGidColumn];
  const widths = columnWidthsFor(settings, ext ? "ext" : "normal");
  const columns = ["column.name", ...(ext ? ["column.ext", "column.size"] : ["column.size", "column.type"]), "column.modified", ...(["column.permissions", "column.uid", "column.gid"] as const).filter((_, i) => owners[i])] as const;
  const style = { ...themeVariables(settings), "--df-cols": columnTemplate(widths), ...(owners.some(Boolean) ? ownerColumnTemplates(widths, ext, owners) : {}) } as CSSProperties;
  const tint = paneTintVariables(settings.paneColorLeft, settings.paneTintStrength, settings.theme, effectiveAccent(settings));
  return <div className="settings-preview" aria-hidden="true" data-fm={settings.theme} style={style}>
    <div className="pane active"><div className={owners.some(Boolean) ? "list owner-columns" : "list"}>
      <div className={`head${ext ? " ext-split" : ""}`}>{columns.map(label => <span key={label} data-column={label.slice("column.".length)}>{tr(label as "column.name")}</span>)}</div>
      <div className="list-body" data-tint={settings.paneColorLeft || undefined} style={tint as CSSProperties}>
        {[tr("settings.sampleFolder"), "Notes.txt", "Image.png"].map((name, i) => {
          const size = <span className="num" data-column="size">{i === 0 ? "—" : i === 1 ? "4 KB" : "128 KB"}</span>;
          return <div key={name} className={`row${ext ? " ext-split" : ""}${i === 0 ? " d" : i === 1 ? " cur sel" : ""}`}>
            <span className="n"><Icon name={i === 0 ? "dir" : "file"} /><span className="nm">{ext && i > 0 ? name.split(".")[0] : name}</span></span>
            {ext ? <><span className="ex" data-column="ext">{i === 0 ? "" : name.split(".")[1]}</span>{size}</> : <>{size}<span className="lt" data-column="type">{tr(i === 0 ? "type.folder" : "type.file")}</span></>}
            <span className="dt" data-column="modified">20.11.2026</span>
            {owners[0] && <span className="owner permissions">{i === 0 ? "rwxr-xr-x" : "rw-r--r--"}</span>}
            {owners[1] && <span className="owner">1000</span>}{owners[2] && <span className="owner">1000</span>}
          </div>;
        })}
      </div>
    </div></div>
  </div>;
}
