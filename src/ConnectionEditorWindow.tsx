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
  draft?: ConnectionProfile;
}

/** Reads the query the main window built (`connection_editor_open`); anything odd falls back to defaults. */
export function parseEditorParams(search: string): EditorParams {
  const q = new URLSearchParams(search);
  const profileId = q.get("profile") || null;
  const draft = profileId ? undefined : discoveredDraft(q.get("discovered"));
  return { profileId, appearance: parseAppearance(search), ...(draft ? { draft } : {}) };
}

/** The native editor supplies a validated cache record; still bound query data in browser previews. */
function discoveredDraft(raw: string | null): ConnectionProfile | undefined {
  if (!raw || raw.length > 4096) return;
  try {
    const d = JSON.parse(raw) as Record<string, unknown>;
    const text = (v: unknown, limit: number): v is string => typeof v === "string" && v.length <= limit && !/[\p{Cc}\p{Cf}\u034f]/u.test(v);
    if (!text(d.name, 63) || !d.name.trim() || !text(d.host, 254) || !d.host || !/^[a-zA-Z0-9.:%_-]+$/.test(d.host)
      || typeof d.port !== "number" || !Number.isInteger(d.port) || d.port < 1 || d.port > 65535
      || !["smb", "nfs", "sftp", "webdav", "webdavs"].includes(String(d.protocol))
      || (d.path != null && (!text(d.path, 256) || /[\\?#@%:]/.test(d.path) || d.path.split("/").some(p => p === "." || p === "..")))) return;
    const path = typeof d.path === "string" ? d.path : undefined;
    const [share, ...folder] = (path ?? "").split("/");
    return { ...newSshProfile(d.host), name: d.name, transport: "builtin", port: d.port,
      protocol: (d.protocol === "webdavs" ? "webdav" : d.protocol) as ConnectionProfile["protocol"],
      tls: d.protocol === "webdavs" ? "explicit" : "none",
      ...(d.protocol === "smb" || d.protocol === "webdav" || d.protocol === "webdavs" ? { share: d.protocol === "smb" ? share : path } : {}),
      ...(d.protocol === "nfs" ? { export: path } : {}),
      startFolder: d.protocol === "sftp" ? path ?? null : d.protocol === "smb" && folder.length ? `/${folder.join("/")}` : null,
    };
  } catch { return; }
}

export function parseAppearance(search: string): EditorAppearance {
  const q = new URLSearchParams(search);
  const pick = <T extends string>(value: string | null, allowed: readonly T[], fallback: T): T => allowed.includes(value as T) ? value as T : fallback;
  const size = Number(q.get("fontSize"));
  return {
    theme: pick(q.get("theme"), ["schiefer", "papier", "werkstatt", "weiss"], "schiefer"),
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
    if (!params.profileId) { setLoad({ status: "ready", profile: params.draft ?? newSshProfile() }); return; }
    api.connectionsList().then((entries) => {
      if (!active) return;
      const found = entries.find((e) => e.profile.id === params.profileId);
      setLoad(found ? { status: "ready", profile: found.profile } : { status: "missing" });
    }).catch(() => { if (active) setLoad({ status: "missing" }); });
    return () => { active = false; };
  }, [api, params.profileId, params.draft]);

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
