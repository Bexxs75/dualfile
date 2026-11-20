import { expect, test } from "vitest";
import { paneTintVariables } from "./paneTint";
import { contrastRatio, defaultAccent, themes } from "./tokens";

test("white theme meets AA on panel, selection and cursor", () => {
  const t = themes.weiss;
  expect(t.panel).toBe("#ffffff");
  expect(defaultAccent("weiss")).toBe(t.accent);
  for (const fg of [t.fg, t.muted, t.directory]) for (const bg of [t.panel, t.selection, t.current]) expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(4.5);
});
test("opaque Midnight Commander blue and pale gold choose readable text", () => {
  const blue = paneTintVariables("#004080", 100, "weiss");
  expect(blue["--panel"]).toBe("#004080");
  expect(blue["--dir"]).toBe("#80ffff");
  const gold = paneTintVariables("#ffe0a0", 100, "schiefer");
  expect(gold["--fg"]).toBe("#1b1f24");
});
test("all text stays AA across themes, strengths and the RGB cube, with visible cursor", () => {
  for (const theme of Object.keys(themes) as (keyof typeof themes)[]) {
    for (const strength of [80, 85, 90, 95, 100]) {
      for (let r = 0; r <= 255; r += 51) for (let g = 0; g <= 255; g += 51) for (let b = 0; b <= 255; b += 51) {
        const color = "#" + [r, g, b].map(v => v.toString(16).padStart(2, "0")).join("");
        const v = paneTintVariables(color, strength, theme);
        for (const fg of ["--fg", "--muted", "--dir"]) for (const bg of ["--panel", "--sel", "--cur"]) expect(contrastRatio(v[fg]!, v[bg]!), `${color}/${strength}/${fg}/${bg}`).toBeGreaterThanOrEqual(4.5);
        expect(v["--cur"]).not.toBe(v["--panel"]);
      }
    }
  }
});
test("no tint leaves theme intact and old strengths default to full opacity", () => {
  expect(paneTintVariables(undefined, 100, "weiss")).toEqual({});
  expect(paneTintVariables("garbage", 100, "weiss")).toEqual({});
  expect(paneTintVariables("#004080", undefined, "weiss")["--panel"]).toBe("#004080");
});
