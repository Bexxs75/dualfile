import { contrastRatio, inkForAccent, isValidHex, themes, type ThemeName } from "./tokens";

/** sRGB mixing matches the colour picker: 100% preserves the chosen hex exactly. */
export function mix(a: string, b: string, fraction: number): string {
  return "#" + [1, 3, 5].map(i => Math.round(
    parseInt(a.slice(i, i + 2), 16) * (1 - fraction) + parseInt(b.slice(i, i + 2), 16) * fraction,
  ).toString(16).padStart(2, "0")).join("");
}

/** `accent` is the effective accent of the app (see effectiveAccent); defaults to the design accent. */
export function paneTintVariables(color: string | undefined, strength: number | undefined, theme: ThemeName, accent?: string): Record<string, string> {
  if (!color || !isValidHex(color)) return {};
  const amount = Number.isFinite(strength) ? Math.max(80, Math.min(100, strength!)) : 100;
  const panel = mix((themes[theme] ?? themes.schiefer).panel, color, amount / 100);
  const light = contrastRatio(panel, "#000000") >= contrastRatio(panel, "#ffffff");
  const ink = light ? "#000000" : "#ffffff";
  // Move away from the contrast boundary; near black/white move inward instead.
  const edge = light ? "#ffffff" : "#000000";
  const inward = contrastRatio(panel, edge) < 1.3;
  const target = inward ? ink : edge;
  const selection = mix(panel, target, inward ? 0.08 : 0.25);
  const current = mix(panel, target, inward ? 0.16 : 0.5);
  const surfaces = [panel, selection, current];
  const readable = (preferred: string) => surfaces.every(bg => contrastRatio(preferred, bg) >= 4.5) ? preferred : ink;
  const tokens = themes[theme] ?? themes.schiefer;
  const fg = readable(light ? "#1b1f24" : "#f2f6fa");
  const wanted = accent && isValidHex(accent) ? accent.toLowerCase() : tokens.accent;
  const paneAccent = contrastRatio(wanted, panel) >= 3 ? wanted : fg;
  return {
    "--warn": readable(tokens.warning),
    "--accent": paneAccent,
    "--accent-ink": paneAccent === fg ? (light ? "#ffffff" : "#000000") : inkForAccent(paneAccent, theme),
    "--panel": panel, "--bg": panel, "--panel2": selection, "--raised": selection,
    "--fg": fg,
    "--muted": readable(light ? "#424a53" : "#d6e4ef"),
    "--dir": readable(light ? "#163c70" : "#80ffff"),
    "--sel": selection, "--cur": current, "--accent-soft": selection,
    "--line": mix(panel, ink, 0.5), "--line-soft": mix(panel, ink, 0.3),
  };
}
