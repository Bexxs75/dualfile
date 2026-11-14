import { expect, test } from "vitest";
import { createMockApi } from "./mock";
import { ManualScheduler, HOME } from "../test-utils";
const options = {
  minSize: 0,
  maxDistance: 6
};
async function fixture() {
  const scheduler = new ManualScheduler(),
    api = createMockApi({
      scheduler
    });
  const id = await api.similarStart([HOME], options);
  await scheduler.runAll();
  return {
    api,
    scheduler,
    id,
    page: await api.similarPage(id, 0, 50)
  };
}
test("similar validates local roots, threshold, held limit, close and pagination", async () => {
  const {
    api,
    scheduler,
    id,
    page
  } = await fixture();
  expect(page.groups.length).toBeGreaterThan(0);
  for (const p of ["archive:/a!/", "ftp:id!/", "sftp:id!/", "conn:id!/", "plugin:id!/"]) await expect(api.similarStart([p], options)).rejects.toMatchObject({
    kind: "invalidInput"
  });
  for (const maxDistance of [-1, 17, 1.5]) await expect(api.similarStart([HOME], {
    ...options,
    maxDistance
  })).rejects.toMatchObject({
    kind: "invalidInput"
  });
  const b = await api.similarStart([HOME], options);
  await expect(api.similarStart([HOME], options)).rejects.toMatchObject({
    kind: "invalidInput"
  });
  await api.similarClose(b);
  await scheduler.runAll();
  await expect(api.similarPage(b, 0, 50)).rejects.toMatchObject({
    kind: "notFound"
  });
  expect((await api.similarPage(id, 999, 50)).groups).toEqual([]);
  await expect(api.similarPage(id, 0, 0)).rejects.toMatchObject({
    kind: "invalidInput"
  });
});
test("explicit choices reject foreign paths, whole groups and duplicates; one consumed trash job supports undo", async () => {
  const {
    api,
    scheduler,
    id,
    page
  } = await fixture();
  const g = page.groups[0]!;
  const path = g.entries[0]!.path;
  const choice = {
    groupIndex: 0,
    trashPaths: [path]
  };
  for (const choices of [[{
    ...choice,
    trashPaths: ["/foreign"]
  }], [{
    ...choice,
    trashPaths: g.entries.map(e => e.path)
  }], [{
    ...choice,
    trashPaths: [path, path]
  }], [choice, choice]]) await expect(api.similarTrash(id, choices)).rejects.toMatchObject({
    kind: "invalidInput"
  });
  const before = (await api.jobs()).length;
  const result = await api.similarTrash(id, [choice]);
  await expect(api.similarTrash(id, [choice])).rejects.toMatchObject({
    kind: "notFound"
  });
  await scheduler.runAll();
  expect((await api.jobs()).length).toBe(before + 1);
  expect((await api.jobs()).find(j => j.id === result.jobId)?.state).toBe("done");
  const history = await api.undoList();
  expect(history).toHaveLength(1);
  const undo = api.undoRun(history[0]!.id);
  await scheduler.runAll();
  await undo;
  const dir = path.slice(0, path.lastIndexOf("/"));
  expect((await api.list(dir)).entries.some(e => e.name === path.split("/").at(-1))).toBe(true);
});
test("suggestions do not remove files; changed thumbnails and trash candidates fail closed", async () => {
  const {
    api,
    scheduler,
    id,
    page
  } = await fixture();
  const g = page.groups[0]!;
  const e = g.entries[0]!;
  for (const rule of (["largestResolution", "largestFile", "oldest", "newest", "shortestPath"] as const)) expect(await api.similarSuggest(id, rule)).toHaveLength(page.total);
  expect((await api.similarThumbnail(id, 0, 0)).bytes.slice(0, 4)).toEqual([137, 80, 78, 71]);
  await expect(api.similarThumbnail(id, 999, 0)).rejects.toMatchObject({
    kind: "invalidInput"
  });
  await api.trash([e.path]);
  await scheduler.runAll();
  await expect(api.similarThumbnail(id, 0, 0)).rejects.toMatchObject({
    kind: "changed"
  });
  expect((await api.similarTrash(id, [{
    groupIndex: 0,
    trashPaths: [e.path]
  }])).skipped).toEqual([{
    path: e.path,
    reason: "changed"
  }]);
});
