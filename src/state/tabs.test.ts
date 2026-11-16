import { describe, expect, it } from "vitest";
import { click, createSelection } from "./selection";
import type { Entry } from "../ipc/types";
import {
  activeTab,
  back,
  closeTab,
  closeOthers,
  toggleLock,
  createWindow,
  forward,
  fromSession,
  goUp,
  navigate,
  newTab,
  nextTab,
  prevTab,
  setFilter,
  setSelection,
  setSort,
  toSession,
} from "./tabs";

const file = (name: string): Entry => ({
  name,
  kind: "file",
  size: 1,
  modified: 0,
  mode: null,
  hidden: false,
  linkTarget: null,
  brokenLink: false,
  error: null,
});

describe("Tabs anlegen und schließen", () => {
  it("newTab dupliziert den aktiven Tab und aktiviert ihn", () => {
    let w = setSort(createWindow("/home/a"), "size:desc");
    w = setFilter(w, "txt");
    w = newTab(w);
    expect(w.tabs).toHaveLength(2);
    expect(w.activeTab).toBe(1);
    expect(activeTab(w)).toMatchObject({ path: "/home/a", sort: "size:desc", filter: "txt" });
  });
  it("newTab fügt direkt neben dem aktiven Tab ein", () => {
    let w = newTab(createWindow("/a"));
    w = navigate(w, "/b");
    w = newTab(w); // [/a, /b, /b]
    w = { ...w, activeTab: 0 };
    w = navigate(w, "/c");
    w = newTab(w);
    expect(w.tabs.map((t) => t.path)).toEqual(["/c", "/c", "/b", "/b"]);
    expect(w.activeTab).toBe(1);
  });
  it("der Duplikat-Tab hat einen eigenen, unabhängigen Verlauf und Auswahl", () => {
    let w = navigate(createWindow("/a"), "/b");
    w = setSelection(w, click(createSelection(), [file("x")], 0));
    w = newTab(w);
    w = navigate(w, "/c");
    expect(w.tabs[0]!.path).toBe("/b");
    expect(w.tabs[0]!.back).toEqual(["/a"]);
    expect([...w.tabs[0]!.selection.selected]).toEqual(["x"]);
  });
  it("schließt den letzten Tab nie", () => {
    const w = createWindow("/a");
    expect(closeTab(w)).toBe(w);
    expect(closeTab(w).tabs).toHaveLength(1);
  });
  it("closeTab hält den richtigen Tab aktiv", () => {
    let w = navigate(newTab(navigate(newTab(createWindow("/1")), "/2")), "/3");
    expect(w.tabs.map((t) => t.path)).toEqual(["/1", "/2", "/3"]);
    expect(w.activeTab).toBe(2);
    w = closeTab(w, 0);
    expect(activeTab(w).path).toBe("/3");
    w = closeTab(w); // aktiven (letzten) schließen
    expect(w.tabs.map((t) => t.path)).toEqual(["/2"]);
    expect(w.activeTab).toBe(0);
  });
  it("closeTab mit ungültigem Index ändert nichts", () => {
    const w = newTab(createWindow("/a"));
    expect(closeTab(w, 7)).toBe(w);
  });
  it("closeOthers behält nur den gewählten Tab", () => {
    const w = navigate(newTab(navigate(newTab(createWindow("/1")), "/2")), "/3");
    const kept = closeOthers(w, 1);
    expect(kept.tabs.map((t) => t.path)).toEqual(["/2"]);
    expect(kept.activeTab).toBe(0);
  });
  it("closeOthers lässt einen einzelnen Tab unverändert", () => {
    const w = createWindow("/a");
    expect(closeOthers(w, 0)).toBe(w);
  });
  it("toggleLock sperrt den Tab auf den Ordner und verhindert das Hochgehen", () => {
    let w = navigate(createWindow("/a"), "/a/b");
    w = toggleLock(w, 0);
    expect(w.tabs[0]!.lockRoot).toBe("/a/b");
    w = goUp(w); // nach /a wäre oberhalb der Sperre → bleibt
    expect(activeTab(w).path).toBe("/a/b");
    w = navigate(w, "/a/b/c"); // tiefer ist erlaubt
    expect(activeTab(w).path).toBe("/a/b/c");
    w = navigate(w, "/other"); // außerhalb der Sperre → blockiert
    expect(activeTab(w).path).toBe("/a/b/c");
    w = toggleLock(w, 0); // entsperren
    expect(w.tabs[0]!.lockRoot).toBeUndefined();
    expect(navigate(w, "/other").tabs[0]!.path).toBe("/other");
  });
});

describe("Tabs wechseln", () => {
  it("wechselt zyklisch vor und zurück", () => {
    let w = newTab(newTab(createWindow("/a"))); // 3 Tabs, aktiv 2
    w = nextTab(w);
    expect(w.activeTab).toBe(0);
    w = prevTab(w);
    expect(w.activeTab).toBe(2);
    w = prevTab(prevTab(w));
    expect(w.activeTab).toBe(0);
  });
  it("bleibt bei einem einzigen Tab stehen", () => {
    const w = createWindow("/a");
    expect(nextTab(w).activeTab).toBe(0);
    expect(prevTab(w).activeTab).toBe(0);
  });
});

describe("Navigation und Verlauf", () => {
  it("navigate füllt den Verlauf, leert vor und setzt Filter/Auswahl zurück", () => {
    let w = setFilter(createWindow("/a"), "x");
    w = setSelection(w, click(createSelection(), [file("f")], 0));
    w = navigate(w, "/a/b");
    const t = activeTab(w);
    expect(t.path).toBe("/a/b");
    expect(t.back).toEqual(["/a"]);
    expect(t.filter).toBe("");
    expect(t.selection.selected.size).toBe(0);
  });
  it("derselbe Pfad erzeugt keinen Verlaufseintrag", () => {
    const w = createWindow("/a");
    expect(activeTab(navigate(w, "/a/")).back).toEqual([]);
  });
  it("back/forward laufen durch den Verlauf", () => {
    let w = navigate(navigate(createWindow("/a"), "/b"), "/c");
    w = back(w);
    expect(activeTab(w).path).toBe("/b");
    w = back(w);
    expect(activeTab(w).path).toBe("/a");
    w = back(w); // am Anfang: nichts
    expect(activeTab(w).path).toBe("/a");
    w = forward(forward(w));
    expect(activeTab(w).path).toBe("/c");
    expect(activeTab(forward(w)).path).toBe("/c");
  });
  it("eine neue Navigation nach back verwirft den Vor-Verlauf", () => {
    let w = back(navigate(navigate(createWindow("/a"), "/b"), "/c"));
    w = navigate(w, "/x");
    expect(activeTab(w).forward).toEqual([]);
    expect(activeTab(w).back).toEqual(["/a", "/b"]);
  });
  it("goUp geht eine Ebene hoch und bleibt an der Wurzel auf /", () => {
    let w = goUp(createWindow("/home/a/docs"));
    expect(activeTab(w).path).toBe("/home/a");
    w = goUp(goUp(w));
    expect(activeTab(w).path).toBe("/");
    const root = goUp(w);
    expect(activeTab(root).path).toBe("/");
    expect(activeTab(root).back).toEqual(activeTab(w).back);
  });
  it("Tabs haben getrennte Verläufe", () => {
    let w = newTab(createWindow("/a"));
    w = navigate(w, "/b");
    w = { ...w, activeTab: 0 };
    expect(activeTab(back(w)).path).toBe("/a");
    expect(activeTab(w).back).toEqual([]);
  });
});

describe("Sitzung", () => {
  it("macht einen Roundtrip über WindowSession", () => {
    let w = setFilter(setSort(createWindow("/a"), "modified:desc"), "md");
    w = navigate(newTab(w), "/b");
    const session = toSession(w);
    expect(session).toEqual({
      tabs: [
        { path: "/a", sort: "modified:desc", filter: "md" },
        { path: "/b", sort: "modified:desc", filter: "" },
      ],
      activeTab: 1,
    });
    expect(toSession(fromSession(session, "/h"))).toEqual(session);
  });
  it("speichert keine direkten Serveradressen, gespeicherte Verbindungen schon", () => {
    let w = navigate(newTab(createWindow("/a")), "sftp://otto@server/home");
    w = navigate(newTab(w), "conn:abc/docs");
    w = navigate(newTab(w), "/b");
    w = { ...w, activeTab: 3 };
    expect(toSession(w)).toEqual({
      tabs: [
        { path: "/a", sort: "name", filter: "" },
        { path: "conn:abc/docs", sort: "name", filter: "" },
        { path: "/b", sort: "name", filter: "" },
      ],
      activeTab: 2,
    });
    expect(toSession({ ...w, activeTab: 1 }).activeTab).toBe(0);
  });
  it("repariert leere Sitzung und ungültigen Aktiv-Index", () => {
    expect(toSession(fromSession({ tabs: [], activeTab: 3 }, "/home"))).toEqual({
      tabs: [{ path: "/home", sort: "name", filter: "" }],
      activeTab: 0,
    });
    const w = fromSession({ tabs: [{ path: "/a", sort: "name", filter: "" }], activeTab: 9 }, "/h");
    expect(w.activeTab).toBe(0);
  });
});
