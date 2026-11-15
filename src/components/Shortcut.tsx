import { Keycap } from "./Keycap";

/** Renders a keymap spec ("Ctrl+Shift+P", "F3", "") as keycaps. Empty spec renders nothing. */
// shortcut: "+" is treated purely as a separator — no current binding uses "+" as a key
//           (+ - * are structurally fixed, not rebindable). Revisit if that ever changes.
export function Shortcut({ spec }: { spec: string }) {
  if (!spec) return null;
  return (
    <span className="sc">
      {spec.split("+").map((key, i) => (
        <span key={i} className="sc-key">
          {i > 0 && <span className="sc-plus">+</span>}
          <Keycap label={key} />
        </span>
      ))}
    </span>
  );
}
