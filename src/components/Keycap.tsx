export type KeycapLabel = string | { top: string; bot: string };

/** Single keycap drawn in pure CSS. A string is a plain label; {top, bot} is a two-line legend. */
export function Keycap({ label, glyph }: { label: KeycapLabel; glyph?: boolean }) {
  if (typeof label === "string") return <span className={glyph ? "kc glyph" : "kc"}>{label}</span>;
  return <span className="kc glyph"><span className="leg"><span className="top">{label.top}</span><span className="bot">{label.bot}</span></span></span>;
}
