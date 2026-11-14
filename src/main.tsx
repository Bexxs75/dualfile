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
// Nur für die Sichtprüfung (VITE_DEMO=1 beim Bauen); im normalen Bau entfällt dieser Zweig vollständig.
const runtime = import.meta.env.VITE_DEMO ? createRuntime(api) : undefined;
createRoot(document.getElementById("root")!).render(<App api={api} runtime={runtime} />);
if (runtime) void import("./devDemo").then((demo) => demo.runDemo(runtime));
