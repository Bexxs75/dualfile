import type { CSSProperties, ReactNode } from "react";
import { useAppState } from "../app/context";
import type { WinIdx } from "../app/state";
import { paneTintVariables } from "../themes/paneTint";
import { effectiveAccent } from "../themes/tokens";

/** Scope every tint token to rows and the remaining list surface, below the header. */
export function ListBody({ w, children }: { w: WinIdx; children: ReactNode }) {
  const settings = useAppState(state => state.settings);
  const color = w === 0 ? settings.paneColorLeft : settings.paneColorRight;
  return <div className="list-body" role="rowgroup"
    {...(color ? { "data-tint": "", style: paneTintVariables(color, settings.paneTintStrength, settings.theme, effectiveAccent(settings)) as CSSProperties } : {})}>
    {children}
  </div>;
}
