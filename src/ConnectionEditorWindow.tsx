import { useEffect, useMemo, useState } from "react";
import { RuntimeProvider, useT } from "./app/context";
import { createRuntime } from "./app/runtime";
import type { DualfileApi } from "./ipc/api";
import type { ConnectionProfile, EditorAppearance, Settings } from "./ipc/types";
import { ConnectionProfileForm, newSshProfile } from "./components/dialogs/ConnectionProfileForm";
import "./components/dialogs/ssh.css";
import { applyTheme } from "./themes/tokens";

export interface EditorParams {
  profileId: string | null;
  appearance: EditorAppearance;
}

/** Reads the query the main window built (`connection_editor_open`); anything odd falls back to defaults. */
export function parseEditorParams(search: string): EditorParams {
  return { profileId: new URLSearchParams(search).get("profile") || null, appearance: parseAppearance(search) };
}

export function parseAppearance(search: string): EditorAppearance {
  const q = new URLSearchParams(search);
  const pick = <T extends string>(value: string | null, allowed: readonly T[], fallback: T): T => allowed.includes(value as T) ? value as T : fallback;
  const size = Number(q.get("fontSize"));
  return {
    theme: pick(q.get("theme"), ["schiefer", "papier", "werkstatt"], "schiefer"),
    accent: q.get("accent") ?? "",
    accentAutomatic: q.get("accentAutomatic") === "1",
    density: pick(q.get("density"), ["compact", "normal", "comfortable"], "normal"),
    fontSize: Number.isFinite(size) && size > 0 ? size : 13,
    lang: pick(q.get("lang"), ["de", "en"], "de"),
  };
}

type Load = { status: "loading" } | { status: "ready"; profile: ConnectionProfile } | { status: "missing" };

function Editor({ api, params }: { api: DualfileApi; params: EditorParams }) {
  const tr = useT();
  const [load, setLoad] = useState<Load>({ status: "loading" });
  const close = () => { void api.connectionEditorClose().catch(() => window.close()); };
  const { theme, accent, accentAutomatic, density, fontSize, lang } = params.appearance;

  useEffect(() => {
    // The shared stylesheet forces a 900x560 minimum on the page; this window has to shrink below it.
    document.documentElement.classList.add("editor-window-root");
    document.documentElement.lang = lang;
    applyTheme(document.documentElement, { theme, accent, accentAutomatic, density, fontSize });
  }, [theme, accent, accentAutomatic, density, fontSize, lang]);

  useEffect(() => {
    let active = true;
    if (!params.profileId) { setLoad({ status: "ready", profile: newSshProfile() }); return; }
    api.connectionsList().then((entries) => {
      if (!active) return;
      const found = entries.find((e) => e.profile.id === params.profileId);
      setLoad(found ? { status: "ready", profile: found.profile } : { status: "missing" });
    }).catch(() => { if (active) setLoad({ status: "missing" }); });
    return () => { active = false; };
  }, [api, params.profileId]);

  // The form mounts only after the profile is loaded, so focus has to wait for that.
  useEffect(() => {
    if (load.status !== "loading") document.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [load.status]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !event.defaultPrevented) close(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [api]);

  return <main className="editor-window" aria-label={tr("ssh.profile")} aria-busy={load.status === "loading"}>
    <div className="editor-scroll">
      {load.status === "ready" && <ConnectionProfileForm api={api} initial={load.profile} onSaved={close} onCancel={close} />}
      {load.status === "missing" && <div className="ssh-profile">
        <p role="alert">{tr("ssh.editorNotFound")}</p>
        <div className="ssh-actions"><button type="button" data-autofocus onClick={close}>{tr("action.close")}</button></div>
      </div>}
    </div>
  </main>;
}

/** Root of the connection editor window: its own runtime that never starts (no events, no settings read or save). */
export function ConnectionEditorWindow({ api, search }: { api: DualfileApi; search: string }) {
  const params = useMemo(() => parseEditorParams(search), [search]);
  const runtime = useMemo(() => {
    const rt = createRuntime(api);
    const { theme, accent, accentAutomatic, density, fontSize, lang } = params.appearance;
    rt.store.dispatch({ type: "settings", update: (s: Settings): Settings => ({ ...s, theme, accent, accentAutomatic, density, fontSize, language: lang }) });
    return rt;
  }, [api, params]);
  return <RuntimeProvider runtime={runtime}><Editor api={api} params={params} /></RuntimeProvider>;
}
