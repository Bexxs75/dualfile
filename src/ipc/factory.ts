import type { DualfileApi } from "./api";
import { createMockApi } from "./mock";
import { createTauriApi } from "./tauri";

/**
 * Im Tauri-Fenster (dort gibt es `__TAURI_INTERNALS__`) die echte API, sonst die Mock-API:
 * so bleibt der Browser-Entwurf (`npm run dev`) ohne Kern nutzbar.
 */
export function createApi(host: object = window): DualfileApi {
  if ("__TAURI_INTERNALS__" in host) return createTauriApi();
  const api = createMockApi({ samples: true });
  let editor: Window | null = null;
  // Browser preview of the two advertised NAS hosts; the native app uses the cache-key command.
  api.connectionEditorOpen = async (_profileId, _title, appearance, discoveryKey) => {
    if (!discoveryKey) return true;
    if (editor && !editor.closed) { editor.focus(); return false; }
    const service = (await api.placesSnapshot()).discoveredServices?.find(s => s.key === discoveryKey);
    if (!service) return false;
    const q = new URLSearchParams({ view: "connection-editor", discovered: JSON.stringify(service),
      ...Object.fromEntries(Object.entries(appearance).map(([key, value]) => [key, key === "accentAutomatic" ? value ? "1" : "0" : String(value)])) });
    editor = window.open(`${window.location.pathname}?${q}`, "dualfile-discovery-editor", "width=520,height=640");
    return !!editor;
  };
  api.connectionEditorClose = async () => { if (new URLSearchParams(window.location.search).get("view") === "connection-editor") window.close(); };
  return api;
}
