import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime } from "../app/runtime";
import { createMockApi } from "../ipc/mock";
import { TerminalShortcutHint } from "./ShortcutHint";

async function setup(language: "de" | "en") {
  const api = createMockApi({ initialSettings: { version: 1, verifyPolicy: "auto", theme: "schiefer", accent: "#e0a45c", density: "normal", fontSize: 13, language, bookmarks: [], session: { windows: [], activeWindow: 0 } } });
  const rt = createRuntime(api, { toastMs: 50 });
  await rt.controller.start();
  return render(<RuntimeProvider runtime={rt}><TerminalShortcutHint><button type="button" title="Fallback">T</button></TerminalShortcutHint></RuntimeProvider>);
}

describe("ShortcutHint", () => {
  test("de: Strg + ^/° keycap and German title", async () => {
    const { container } = await setup("de");
    const card = container.querySelector(".hovercard")!;
    expect(card.textContent).toContain("Terminal ein-/ausschalten");
    const caps = [...card.querySelectorAll(".kc")];
    expect(caps[0].textContent).toBe("Strg");
    expect(caps[1].querySelector(".top")?.textContent).toBe("^");
    expect(caps[1].querySelector(".bot")?.textContent).toBe("°");
    expect(screen.getByRole("button").getAttribute("title")).toBe("Fallback");
  });

  test("en: Ctrl + backquote keycap and English title", async () => {
    const { container } = await setup("en");
    const card = container.querySelector(".hovercard")!;
    expect(card.textContent).toContain("Toggle terminal");
    const caps = [...card.querySelectorAll(".kc")];
    expect(caps[0].textContent).toBe("Ctrl");
    expect(caps[1].textContent).toBe("`");
    expect(caps[1].querySelector(".leg")).toBeNull();
  });
});
