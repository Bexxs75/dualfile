import { describe, expect, test } from "vitest";
import { createMockApi } from "./mock";
import { mockKeep } from "./spaceMock";
import { ManualScheduler, HOME } from "../test-utils";
import type { DupeEntry, KeepRule } from "./types";
const ROOT = `${HOME}/Vergleich/links`;
function setup() {
  const scheduler = new ManualScheduler();
  return {
    scheduler,
    api: createMockApi({
      scheduler,
      compareDemo: true
    })
  };
}
describe("local scan mock", () => {
  test("rejects nonlocal paths with a fixed error", async () => {
    const {
      api
    } = setup();
    for (const path of ["archive:/a.zip!/", "ftp:id!/", "sftp:id!/", "conn:id!/", "plugin:id!/"]) {
      await expect(api.usageStart(path, {
        oneFileSystem: true
      })).rejects.toMatchObject({
        kind: "invalidInput",
        message: "Nur lokale Ordner"
      });
      await expect(api.dupesStart([path], {
        minSize: 1
      })).rejects.toMatchObject({
        kind: "invalidInput"
      });
    }
  });
  test("limits running sessions, frees them and does not resurrect closed results", async () => {
    const {
      api,
      scheduler
    } = setup();
    const a = await api.usageStart(ROOT, {
      oneFileSystem: true
    });
    const b = await api.usageStart(ROOT, {
      oneFileSystem: true
    });
    await expect(api.usageStart(ROOT, {
      oneFileSystem: true
    })).rejects.toMatchObject({
      kind: "invalidInput"
    });
    await api.usageClose(a);
    const c = await api.usageStart(ROOT, {
      oneFileSystem: true
    });
    await api.usageClose(b);
    await scheduler.runAll();
    await expect(api.usageNode(a)).rejects.toMatchObject({
      kind: "notFound"
    });
    expect((await api.usageNode(c)).ready).toBe(true);
  });
  test("usage drilldown stays in held result", async () => {
    const {
      api,
      scheduler
    } = setup();
    const id = await api.usageStart(ROOT, {
      oneFileSystem: true
    });
    expect((await api.usageNode(id)).ready).toBe(false);
    await scheduler.runAll();
    const page = await api.usageNode(id);
    expect(page.totals!.files).toBeGreaterThan(0);
    const dir = page.children.find(e => e.totals.directories > 0)!;
    expect((await api.usageNode(id, dir.path)).path).toBe(dir.path);
    await expect(api.usageNode(id, "/etc")).rejects.toMatchObject({
      kind: "invalidInput"
    });
  });
  test("duplicate selections are backend-derived, one trash job has one undo record", async () => {
    const {
      api,
      scheduler
    } = setup();
    const id = await api.dupesStart([ROOT, `${HOME}/Vergleich/rechts`], {
      minSize: 1
    });
    await scheduler.runAll();
    const page = await api.dupesPage(id, 0, 100);
    expect(page.total).toBeGreaterThan(0);
    expect((await api.dupesPage(id, 1, 1)).groups).toHaveLength(1);
    await expect(api.dupesTrash(id, [{
      groupIndex: 0,
      keepPath: "/etc/passwd"
    }])).rejects.toMatchObject({
      kind: "invalidInput"
    });
    const choices = await api.dupesSuggest(id, "oldest", ROOT);
    const before = (await api.jobs()).length;
    const result = await api.dupesTrash(id, choices);
    expect((await api.jobs()).length).toBe(before + 1);
    expect((await api.jobs()).find(j => j.id === result.jobId)?.state).toBe("done");
    const history = await api.undoList();
    expect(history).toHaveLength(1);
    for (const choice of choices) await expect(api.fileInfo(choice.keepPath)).resolves.toBeDefined();
    const undo = api.undoRun(history[0]!.id);
    await scheduler.runAll();
    await undo;
  });
  test("changed duplicates are skipped", async () => {
    const {
      api,
      scheduler
    } = setup();
    const id = await api.dupesStart([ROOT, `${HOME}/Vergleich/rechts`], {
      minSize: 1
    });
    await scheduler.runAll();
    const page = await api.dupesPage(id, 0, 1),
      g = page.groups[0]!;
    const removed = g.entries[1]!.path;
    await api.trash([removed]);
    const result = await api.dupesTrash(id, [{
      groupIndex: g.groupIndex,
      keepPath: g.entries[0]!.path
    }]);
    expect(result.skipped).toContainEqual({
      path: removed,
      reason: "changed"
    });
  });
  test("pause and cancellation use the job manager", async () => {
    const {
      api,
      scheduler
    } = setup();
    const id = await api.dupesStart([ROOT], {
      minSize: 1
    });
    await Promise.resolve();
    await api.jobPause(id);
    expect((await api.jobs()).find(j => j.id === id)?.state).toBe("paused");
    await api.cancelJob(id);
    await scheduler.runAll();
    expect((await api.jobs()).find(j => j.id === id)?.state).toBe("cancelled");
    await api.dupesClose(id);
  });
});
describe("mock keep rules match native core tie rules", () => {
  const e = (path: string, modified: number | null): DupeEntry => ({
    path,
    modified,
    protected: false
  });
  test.each<KeepRule>(["oldest", "newest", "shortestPath", "inFolder"])("%s uses smallest path on ties", rule => {
    expect(mockKeep([e("/b", 1), e("/a", 1)], rule, "/else")?.path).toBe("/a");
  });
  test("known dates precede unknown, newest and oldest differ", () => {
    const rows = [e("/a", null), e("/b", 1), e("/c", 2)];
    expect(mockKeep(rows, "oldest", "/")?.path).toBe("/b");
    expect(mockKeep(rows, "newest", "/")?.path).toBe("/c");
  });
  test("native UTF-8 length and component-aware folder membership", () => {
    expect(mockKeep([e("/é", 0), e("/ab", 0)], "shortestPath", "/")?.path).toBe("/ab");
    expect(mockKeep([e("/a-file", 0), e("/a/z", 0)], "inFolder", "/a")?.path).toBe("/a/z");
  });
});
