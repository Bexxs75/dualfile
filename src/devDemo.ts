// Nur für die Sichtprüfung im Entwurf (Bauen mit VITE_DEMO=1): öffnet per URL-Parameter eine
// Beispieldatei im Lister oder in der Schnellansicht. Wird im normalen Bau nicht eingebunden.
import type { AppRuntime } from "./app/runtime";
import { visibleEntries } from "./app/state";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export async function runDemo(runtime: AppRuntime): Promise<void> {
  const params = new URLSearchParams(location.search);
  const file = params.get("demo");
  const theme = params.get("theme");
  const accent = params.get("accent");
  const tint = params.get("tint");
  const strength = params.get("strength");
  const owners = params.has("owners");
  if (!file && !theme && !accent && !tint && !owners && !params.has("settings")) return;
  const { store, controller } = runtime;
  while (store.getState().phase !== "ready") await sleep(50);
  if (theme === "schiefer" || theme === "papier" || theme === "werkstatt" || theme === "weiss") controller.updateSettings((s) => ({ ...s, theme }));
  if (accent) controller.updateSettings((s) => ({ ...s, accent, accentAutomatic: false }));
  if (tint) controller.updateSettings((s) => ({ ...s, paneColorLeft: "#" + tint, ...(strength ? { paneTintStrength: Number(strength) } : {}) }));
  if (owners) controller.updateSettings((s) => ({ ...s, showPermissionsColumn: true, showUidColumn: true, showGidColumn: true }));
  if (params.has("settings")) {
    controller.openDialog({ kind: "settings" });
    return;
  }
  if (!file) return;
  await controller.navigateTo(0, `${store.getState().home}/Ansicht`);
  const index = visibleEntries(store.getState(), 0).findIndex((entry) => entry.name === file);
  if (index < 0) return;
  controller.clickRow(0, index, { ctrl: false, shift: false });
  controller.command(params.get("quick") ? "quickView" : "view");
  await sleep(600);
  for (const key of params.get("keys") ?? "") {
    (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    await sleep(300);
  }
}
