import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles/app.css";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createRuntime } from "./app/runtime";
import { createApi } from "./ipc/factory";

// Im Tauri-Fenster trägt der Rust-Kern die API; im Browser-Entwurf die Mock-API.
const api = createApi();
const root = createRoot(document.getElementById("root")!);
if (new URLSearchParams(location.search).get("view") === "connection-editor") {
  // Second OS window: same bundle, only the editor is loaded and the main app never starts.
  void import("./ConnectionEditorWindow").then(({ ConnectionEditorWindow }) => root.render(<ConnectionEditorWindow api={api} search={location.search} />));
} else {
  // Nur für die Sichtprüfung (VITE_DEMO=1 beim Bauen); im normalen Bau entfällt dieser Zweig vollständig.
  const runtime = import.meta.env.VITE_DEMO ? createRuntime(api) : undefined;
  root.render(<App api={api} runtime={runtime} />);
  if (runtime) void import("./devDemo").then((demo) => demo.runDemo(runtime));
}
