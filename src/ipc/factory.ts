import type { DualfileApi } from "./api";
import { createMockApi } from "./mock";
import { createTauriApi } from "./tauri";

/**
 * Im Tauri-Fenster (dort gibt es `__TAURI_INTERNALS__`) die echte API, sonst die Mock-API:
 * so bleibt der Browser-Entwurf (`npm run dev`) ohne Kern nutzbar.
 */
export function createApi(host: object = window): DualfileApi {
  return "__TAURI_INTERNALS__" in host ? createTauriApi() : createMockApi({ samples: true });
}
