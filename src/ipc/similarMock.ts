import { CoreError, type DualfileApi } from "./api";
import type { Hooks } from "./spaceMock";
import type { SimilarPage, SimilarEntry, SimilarRule } from "./types";
type Api = Pick<DualfileApi, "similarStart" | "similarPage" | "similarClose" | "similarSuggest" | "similarThumbnail" | "similarTrash">;
export function similarKeep(entries: SimilarEntry[], rule: SimilarRule) {
  return [...entries].sort((a, b) => {
    let order = 0;
    if (rule === "largestResolution") order = b.width * b.height - a.width * a.height;
    if (rule === "largestFile") order = b.size - a.size;
    if (rule === "shortestPath") order = new TextEncoder().encode(a.path).length - new TextEncoder().encode(b.path).length;
    if (rule === "oldest" || rule === "newest") order = a.modified === null ? b.modified === null ? 0 : 1 : b.modified === null ? -1 : (a.modified - b.modified) * (rule === "oldest" ? 1 : -1);
    return order || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  })[0]!;
}
export function createSimilarMock(h: Hooks & { cancelled(id: number): boolean }): Api {
  const held = new Map<number, SimilarPage>();
  const errors = new Map<number, unknown>();
  const gone = () => new CoreError("Das Messergebnis ist nicht (mehr) vorhanden", "notFound");
  const invalid = () => new CoreError("Ungültige Scan-Auswahl", "invalidInput");
  const get = (id: number) => {
    const p = held.get(id);
    if (!p) throw gone();
    return p;
  };
  const valid = (e: SimilarEntry) => {
    try {
      const live = h.entry(e.path);
      return live.kind === "file" && live.size === e.size && live.modified === e.modified;
    } catch {
      return false;
    }
  };
  let thumbnails = 0;
  return {
    async similarStart(roots, options) {
      if (!roots.length || roots.length > 64 || held.size >= 2 || !Number.isSafeInteger(options.minSize) || options.minSize < 0 || !Number.isInteger(options.maxDistance) || options.maxDistance < 0 || options.maxDistance > 16) throw invalid();
      roots = roots.map(p => {
        if (/^(archive|ftp|sftp|smb|conn|plugin):/.test(p)) throw new CoreError("Nur lokale Ordner", "invalidInput");
        const r = h.resolve(p);
        if (h.entry(r).kind !== "dir") throw invalid();
        return r;
      });
      const p: SimilarPage = {
        ready: false,
        progress: {
          stage: "scan",
          entries: 0,
          bytes: 0,
          total: null
        },
        total: 0,
        images: 0,
        reclaimableBytes: 0,
        groups: [],
        issues: [],
        incomplete: false,
        cancelled: false
      };
      const id = h.start("Ähnliche Bilder", "similar", async id => {
        const entries: SimilarEntry[] = [];
        const seen = new Set<string>();
        const walk = async (path: string): Promise<void> => {
          await h.checkpoint(id);
          if (seen.has(path)) return;
          seen.add(path);
          const e = h.entry(path);
          p.progress.entries++;
          h.report(id, p.progress.entries, p.progress.bytes);
          if (e.error) {
            p.issues.push([path, "io"]);
            p.incomplete = true;
            return;
          }
          if (e.kind === "dir") {
            for (const c of h.list(path)) await walk(path.replace(/\/$/, "") + "/" + c.name);
          } else if (e.kind === "file" && /\.(png|jpe?g|webp|gif|bmp)$/i.test(path) && e.size >= options.minSize) entries.push({
            path,
            size: e.size,
            modified: e.modified,
            protected: false,
            width: 640,
            height: 480,
            hash: "0000000000000000",
            distanceToReference: 0
          });
        };
        try {
          for (const r of roots) await walk(r);
          p.progress = {
            stage: "hash",
            entries: entries.length,
            bytes: entries.reduce((n, e) => n + e.size, 0),
            total: entries.length
          };
          await h.checkpoint(id);
          p.progress.stage = "group";
          await h.checkpoint(id);
          // Synthetic solid images share a pHash; browser fixtures never decode local files.
          if (entries.length > 1) p.groups = [{
            groupIndex: 0,
            entries,
            reclaimableBytes: entries.reduce((n, e) => n + e.size, 0) - Math.max(...entries.map(e => e.size))
          }];
          p.total = p.groups.length;
          p.images = p.groups.reduce((n, g) => n + g.entries.length, 0);
          p.reclaimableBytes = p.groups.reduce((n, g) => n + g.reclaimableBytes, 0);
          p.ready = true;
        } catch (e) {
          p.incomplete = true;
          p.cancelled = h.cancelled(id);
          p.ready = p.cancelled;
          if (!p.cancelled) errors.set(id, e);
          throw e;
        }
      });
      held.set(id, p);
      return id;
    },
    async similarPage(id, offset, limit) {
      const p = get(id);
      if (errors.has(id)) throw errors.get(id);
      if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(limit) || limit < 1) throw invalid();
      return structuredClone({
        ...p,
        groups: p.groups.slice(offset, offset + Math.min(limit, 100))
      });
    },
    async similarClose(id) {
      get(id);
      held.delete(id);
      errors.delete(id);
      h.cancel(id);
    },
    async similarSuggest(id, rule) {
      const p = get(id);
      if (!p.ready) throw gone();
      return p.groups.map(g => ({
        groupIndex: g.groupIndex,
        keepPath: similarKeep(g.entries, rule).path
      }));
    },
    async similarThumbnail(id, groupIndex, entryIndex) {
      const p = get(id);
      const e = p.groups[groupIndex]?.entries[entryIndex];
      if (!p.ready || !e) throw invalid();
      while (thumbnails >= 2) {
        await new Promise(r => setTimeout(r, 10));
        get(id);
      }
      thumbnails++;
      try {
        get(id);
        if (!valid(e)) throw new CoreError("Datei hat sich geändert", "changed");
        // A one-pixel PNG, transported like the native in-memory thumbnail.
        const bytes = Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNoaGgAAAMEAYFL09IQAAAAAElFTkSuQmCC"), c => c.charCodeAt(0));
        return {
          bytes,
          width: 1,
          height: 1
        };
      } finally {
        thumbnails--;
      }
    },
    async similarTrash(id, choices) {
      const p = get(id);
      if (!p.ready) throw gone();
      const groups = new Set<number>(),
        paths = new Set<string>();
      const selected: SimilarEntry[] = [];
      for (const c of choices) {
        const g = p.groups[c.groupIndex];
        if (!g || groups.has(c.groupIndex) || c.trashPaths.length >= g.entries.length) throw invalid();
        groups.add(c.groupIndex);
        for (const path of c.trashPaths) {
          const e = g.entries.find(e => e.path === path);
          if (!e || e.protected || paths.has(path)) throw invalid();
          paths.add(path);
          selected.push(e);
        }
      }
      p.ready = false;
      const skipped = selected.filter(e => !valid(e)).map(e => ({
        path: e.path,
        reason: "changed"
      }));
      return {
        jobId: await h.trash(selected.filter(valid).map(e => e.path)),
        skipped
      };
    }
  };
}
