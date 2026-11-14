import { CoreError, type DualfileApi } from "./api";
import type { DupeEntry, DupesPage, Entry, KeepRule, UsagePage, UsageTotals } from "./types";

/** Mock's native paths are UTF-8 on Linux. Do not use locale collation for ties. */
export function mockKeep(entries: DupeEntry[], rule: KeepRule, folder: string): DupeEntry | undefined {
  const bytes = (s: string) => new TextEncoder().encode(s);
  const lex = (a: string, b: string) => {
    const x = bytes(a),
      y = bytes(b);
    for (let i = 0; i < Math.min(x.length, y.length); i++) {
      if (x[i] !== y[i]) return x[i]! - y[i]!;
    }
    return x.length - y.length;
  };
  const inside = (p: string) => p === folder || p.startsWith(folder.replace(/\/$/, "") + "/");
  return [...entries].sort((a, b) => {
    let order = 0;
    if (rule === "oldest" || rule === "newest") order = a.modified === null ? b.modified === null ? 0 : 1 : b.modified === null ? -1 : (a.modified - b.modified) * (rule === "oldest" ? 1 : -1);
    if (rule === "shortestPath") order = bytes(a.path).length - bytes(b.path).length;
    if (rule === "inFolder") order = Number(inside(b.path)) - Number(inside(a.path));
    return order || lex(a.path, b.path);
  })[0];
}
type ScanApi = Pick<DualfileApi, "usageStart" | "usageNode" | "usageClose" | "dupesStart" | "dupesPage" | "dupesClose" | "dupesSuggest" | "dupesTrash">;
export interface Hooks {
  resolve(path: string): string;
  list(path: string): Entry[];
  entry(path: string): Entry;
  version(path: string): number;
  start(name: string, kind: string, run: (id: number) => Promise<void>): number;
  checkpoint(id: number): Promise<void>;
  report(id: number, entries: number, bytes: number): void;
  cancel(id: number): void;
  trash(paths: string[]): Promise<number>;
}
export function createSpaceMock(h: Hooks): ScanApi {
  const usage = new Map<number, Map<string, UsagePage>>(),
    dupes = new Map<number, DupesPage>();
  const local = (p: string) => {
    if (/^(archive|ftp|sftp|smb|conn|plugin):/.test(p)) throw new CoreError("Nur lokale Ordner", "invalidInput");
    return h.resolve(p);
  };
  const root = (p: string) => {
    const path = local(p);
    if (h.entry(path).kind !== "dir") throw new CoreError("Der Pfad ist kein Ordner", "notDirectory");
    return path;
  };
  const gone = () => new CoreError("Das Messergebnis ist nicht (mehr) vorhanden", "notFound");
  const invalid = () => new CoreError("Ungültige Scan-Auswahl", "invalidInput");
  const progress = () => ({
    stage: ("size" as const),
    entries: 0,
    bytes: 0,
    total: null
  });
  const totals = (): UsageTotals => ({
    size: 0,
    allocated: 0,
    files: 0,
    directories: 0
  });
  const emptyUsage = (): UsagePage => ({
    ready: false,
    progress: progress(),
    seconds: 0,
    path: "",
    totals: null,
    children: [],
    other: null,
    issues: [],
    skippedCount: 0,
    incomplete: false
  });
  return {
    async usageStart(path) {
      path = root(path);
      if (usage.size >= 2) throw invalid();
      const pages = new Map<string, UsagePage>();
      const pending = emptyUsage();
      pages.set("", pending);
      const id = h.start("Platzbelegung", "space", async id => {
        const began = Date.now();
        const visit = async (p: string): Promise<UsageTotals> => {
          await h.checkpoint(id);
          const e = h.entry(p),
            t = totals();
          pending.progress.entries++;
          if (e.error) {
            pending.issues.push([p, "io"]);pending.skippedCount++;pending.incomplete=true;
            return t;
          }
          h.report(id,pending.progress.entries,pending.progress.bytes);
          const children = [];
          if (e.kind === "dir") {
            t.directories++;
            for (const child of h.list(p)) {
              const cp = p.replace(/\/$/, "") + "/" + child.name,
                ct = await visit(cp);
              children.push({
                path: cp,
                totals: ct
              });
              for (const k of (["size", "allocated", "files", "directories"] as const)) t[k] += ct[k];
            }
          } else {
            t.files = 1;
            t.size = e.size;
            t.allocated = e.size;
            pending.progress.bytes += e.size;
          }
          children.sort((a, b) => b.totals.allocated - a.totals.allocated || (a.path < b.path ? -1 : 1));
          pages.set(p, {
            ...emptyUsage(),
            ready: true,
            path: p,
            totals: t,
            children,
            seconds: (Date.now() - began) / 1000
          });
          return t;
        };
        await visit(path);
        for (const page of pages.values()) {
          page.issues=pending.issues;page.skippedCount=pending.skippedCount;page.incomplete=pending.incomplete;
        }
        pages.set("", pages.get(path)!);
      });
      usage.set(id, pages);
      return id;
    },
    async usageNode(id, path) {
      const pages = usage.get(id);
      if (!pages) throw gone();
      const page = pages.get(path === undefined ? "" : local(path));
      if (!page) throw invalid();
      return structuredClone(page);
    },
    async usageClose(id) {
      if (!usage.delete(id)) throw gone();
      h.cancel(id);
    },
    async dupesStart(roots, options) {
      if (!roots.length || roots.length > 64 || !Number.isSafeInteger(options.minSize) || options.minSize < 0) throw invalid();
      roots = roots.map(root);
      if (dupes.size >= 2) throw invalid();
      const page: DupesPage = {
        ready: false,
        progress: progress(),
        total: 0,
        reclaimableBytes: 0,
        groups: [],
        issues: [],
        incomplete: false
      };
      const id = h.start("Duplikate finden", "dupes", async id => {
        const seen = new Set<string>(),
          groups = new Map<string, {
            size: number;
            entries: DupeEntry[];
          }>();
        const walk = async (p: string): Promise<void> => {
          await h.checkpoint(id);
          if (seen.has(p)) return;
          seen.add(p);
          page.progress.entries++;
          const e = h.entry(p);
          h.report(id,page.progress.entries,page.progress.bytes);
          if(e.error) {page.issues.push([p,"io"]);page.incomplete=true;return;}
          if (e.kind === "dir") {
            for (const c of h.list(p)) await walk(p.replace(/\/$/, "") + "/" + c.name);
          } else if (e.kind === "file" && e.size >= Math.max(1, options.minSize)) {
            const key = `${e.size}:${h.version(p)}`;
            const group = groups.get(key) ?? {
              size: e.size,
              entries: []
            };
            group.entries.push({
              path: p,
              modified: e.modified,
              protected: false
            });
            groups.set(key, group);
          }
        };
        for (const p of roots) await walk(p);
        page.progress.stage = "partialHash";
        await h.checkpoint(id);
        page.progress.stage = "fullHash";
        await h.checkpoint(id);
        page.groups = [...groups.values()].filter(g => g.entries.length > 1).sort((a, b) => b.size * (b.entries.length - 1) - a.size * (a.entries.length - 1)).map((g, i) => ({
          ...g,
          groupIndex: i,
          reclaimableBytes: g.size * (g.entries.length - 1)
        }));
        page.total = page.groups.length;
        page.reclaimableBytes = page.groups.reduce((n, g) => n + g.reclaimableBytes, 0);
        page.ready = true;
      });
      dupes.set(id, page);
      return id;
    },
    async dupesPage(id, offset, limit) {
      const page = dupes.get(id);
      if (!page) throw gone();
      if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(limit) || limit < 1) throw invalid();
      return structuredClone({
        ...page,
        groups: page.groups.slice(offset, offset + Math.min(limit, 100))
      });
    },
    async dupesClose(id) {
      if (!dupes.delete(id)) throw gone();
      h.cancel(id);
    },
    async dupesSuggest(id, rule, folder) {
      folder = local(folder);
      const p = dupes.get(id);
      if (!p?.ready) throw gone();
      return p.groups.map(g => {
        const keepPath = mockKeep(g.entries, rule, folder)!.path;
        const count = g.entries.filter(e => e.path !== keepPath && !e.protected).length;
        return {
          groupIndex: g.groupIndex,
          keepPath,
          redundantCount: count,
          redundantBytes: count * g.size
        };
      });
    },
    async dupesTrash(id, choices) {
      const p = dupes.get(id);
      if (!p?.ready) throw gone();
      const seen = new Set<number>(),
        paths: string[] = [],
        skipped: {
          path: string;
          reason: string;
        }[] = [];
      for (const c of choices) {
        const g = p.groups[c.groupIndex];
        if (!g || seen.has(c.groupIndex) || !g.entries.some(e => e.path === c.keepPath)) throw invalid();
        seen.add(c.groupIndex);
        const valid = (e: DupeEntry) => {
          try {
            const live = h.entry(e.path);
            return live.kind === "file" && live.size === g.size && live.modified === e.modified;
          } catch {
            return false;
          }
        };
        const keeper = g.entries.find(e => e.path === c.keepPath)!;
        for (const e of g.entries) {
          if (e.path === c.keepPath) continue;
          const reason = !valid(keeper) ? "keeperChanged" : e.protected ? "hardlink" : !valid(e) ? "changed" : null;
          if (reason) skipped.push({
            path: e.path,
            reason
          });else paths.push(e.path);
        }
      }
      p.ready = false;
      return {
        jobId: await h.trash(paths),
        skipped
      };
    }
  };
}
