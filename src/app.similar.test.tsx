import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { HOME, setupApp } from "./test-utils";
import { CoreError } from "./ipc/api";
const root = `${HOME}/Bilder`;
const dialog = () => screen.getByRole("dialog", {
  name: "Ähnliche Bilder"
});
test("similar workflow has no defaults, protects last card, explicit suggestion, confirmation, trash and rescan", async () => {
  const trash = vi.fn(),
    start = vi.fn();
  const app = await setupApp({
    left: root,
    wrap: api => ({
      ...api,
      similarStart: (...args) => {
        start();
        return api.similarStart(...args);
      },
      similarTrash: (...args) => {
        trash(...args);
        return api.similarTrash(...args);
      }
    })
  });
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Ähnliche Bilder" }));
  expect(dialog()).toHaveTextContent("Nichts verlässt diesen Computer");
  expect(dialog()).toHaveTextContent("Ähnlich heißt nicht gleich");
  expect(within(dialog()).getByLabelText("Empfindlichkeit")).toHaveValue("6");
  await app.user.click(within(dialog()).getByRole("button", {
    name: "Suche starten"
  }));
  await app.runJobs();
  const boxes = await within(dialog()).findAllByRole("checkbox");
  expect(boxes).toHaveLength(3);
  for (const box of boxes) expect(box).not.toBeChecked();
  const remove = within(dialog()).getByRole("button", {
    name: "In den Papierkorb …"
  });
  expect(remove).toBeDisabled();
  await app.user.click(boxes[0]!);
  await app.user.click(boxes[1]!);
  expect(boxes[2]).toBeDisabled();
  expect(trash).not.toHaveBeenCalled();
  await app.user.click(within(dialog()).getByRole("button", {
    name: "Markierung löschen"
  }));
  for (const box of boxes) expect(box).not.toBeChecked();
  const cards = within(dialog()).getAllByRole("article");
  cards[0]!.focus();
  fireEvent.keyDown(cards[0]!, {
    key: "ArrowRight"
  });
  expect(cards[1]).toHaveFocus();
  fireEvent.keyDown(cards[1]!, {
    key: " "
  });
  expect(boxes[1]).toBeChecked();
  await app.user.click(within(dialog()).getByRole("button", {
    name: "Vorschlag übernehmen"
  }));
  await waitFor(() => expect(boxes.filter(b => (b as HTMLInputElement).checked)).toHaveLength(2));
  expect(trash).not.toHaveBeenCalled();
  await app.user.click(remove);
  const confirm = screen.getByRole("alertdialog");
  expect(confirm).toHaveTextContent("rückgängig");
  expect(confirm).toContainElement((document.activeElement as HTMLElement));
  await app.user.click(within(confirm).getByRole("button", {
    name: "Ja, Papierkorb"
  }));
  await waitFor(() => expect(trash).toHaveBeenCalledTimes(1));
  await app.runJobs();
  await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
  await app.runJobs();
});
test("similar preview failure, incomplete results and changed-file result remain visible", async () => {
  const app = await setupApp({
    left: root,
    wrap: api => ({
      ...api,
      similarThumbnail: async () => {
        throw new CoreError("Datei hat sich geändert", "changed");
      },
      similarPage: async (...args) => ({
        ...(await api.similarPage(...args)),
        incomplete: true,
        issues: [["/broken.png", "invalidImage"]]
      }),
      similarTrash: async (...args) => ({
        ...(await api.similarTrash(...args)),
        skipped: [{
          path: "/changed.png",
          reason: "changed"
        }]
      })
    })
  });
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Ähnliche Bilder" }));
  await app.user.click(within(dialog()).getByRole("button", {
    name: "Suche starten"
  }));
  await app.runJobs();
  // Thumbnails settle independently: finding the first failure does not mean all three settled.
  await waitFor(() => expect(within(dialog()).getAllByRole("img", {
    name: "Vorschau fehlgeschlagen"
  })).toHaveLength(3));
  expect(dialog()).toHaveTextContent("unvollständig");
  await app.user.click(within(dialog()).getAllByRole("checkbox")[0]!);
  await app.user.click(within(dialog()).getByRole("button", {
    name: "In den Papierkorb …"
  }));
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", {
    name: "Ja, Papierkorb"
  }));
  await waitFor(() => expect(dialog()).toHaveTextContent("/changed.png"));
  await app.runJobs();
});
test("English similar dialog traps focus and Escape closes the session", async () => {
  const close = vi.fn();
  const app = await setupApp({
    left: root,
    language: "en",
    wrap: api => ({
      ...api,
      similarClose: id => {
        close(id);
        return api.similarClose(id);
      }
    })
  });
  await app.user.click(screen.getByRole("button", { name: "Tools" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Similar images" }));
  const d = screen.getByRole("dialog", {
    name: "Similar images"
  });
  expect(d).toHaveTextContent("Similar does not mean identical");
  await app.user.click(within(d).getByRole("button", {
    name: "Start search"
  }));
  await app.runJobs();
  await within(d).findAllByRole("checkbox");
  const buttons = within(d).getAllByRole("button");
  const first = buttons[0]!;
  first.focus();
  fireEvent.keyDown(first, {
    key: "Tab",
    shiftKey: true
  });
  expect(d).toContainElement((document.activeElement as HTMLElement));
  expect(first).not.toHaveFocus();
  await app.user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog", {
    name: "Similar images"
  })).not.toBeInTheDocument();
  await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
});
test("previews wait for visibility, run at most two and discard the closed queue", async () => {
  const observers: {
    callback: IntersectionObserverCallback;
    target?: Element;
  }[] = [];
  class Observer {
    constructor(callback: IntersectionObserverCallback) {
      observers.push({
        callback
      });
    }
    observe(target: Element) {
      observers[observers.length - 1]!.target = target;
    }
    disconnect() {}
  }
  vi.stubGlobal("IntersectionObserver", Observer);
  const pending: (() => void)[] = [];
  const requests: Promise<unknown>[] = [];
  const thumbnail = vi.fn();
  try {
    const app = await setupApp({
      left: root,
      wrap: api => ({
        ...api,
        similarThumbnail: (...args) => {
          thumbnail(...args);
          const request = new Promise<Awaited<ReturnType<typeof api.similarThumbnail>>>(resolve => pending.push(() => {
            void api.similarThumbnail(...args).then(resolve, () => resolve({
              bytes: [],
              width: 1,
              height: 1
            }));
          }));
          requests.push(request);
          return request;
        }
      })
    });
    await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Ähnliche Bilder" }));
    await app.user.click(within(dialog()).getByRole("button", {
      name: "Suche starten"
    }));
    await app.runJobs();
    await within(dialog()).findAllByRole("checkbox");
    expect(thumbnail).not.toHaveBeenCalled();
    for (const observer of observers) observer.callback([({
      isIntersecting: true,
      target: observer.target!
    } as IntersectionObserverEntry)], ({} as IntersectionObserver));
    expect(thumbnail).toHaveBeenCalledTimes(2);
    await app.user.keyboard("{Escape}");
    // Drain the actual requests and their React effects before checking the closed queue.
    await act(async () => {
      for (const release of pending) release();
      await Promise.all(requests);
    });
    expect(thumbnail).toHaveBeenCalledTimes(2);
  } finally {
    vi.unstubAllGlobals();
  }
});
test("similar groups load in pages of fifty without marking new entries", async () => {
  const pages = vi.fn();
  const app = await setupApp({
    left: root,
    wrap: api => ({
      ...api,
      similarPage: async (id, offset, limit) => {
        pages(offset, limit);
        const p = await api.similarPage(id, 0, 50);
        if (!p.ready) return p;
        return {
          ...p,
          total: 51,
          images: 153,
          groups: Array.from({
            length: Math.min(50, 51 - offset)
          }, (_, i) => ({
            ...p.groups[0]!,
            groupIndex: offset + i,
            entries: p.groups[0]!.entries.map(e => ({
              ...e,
              path: e.path + `-${offset + i}`
            }))
          }))
        };
      },
      similarThumbnail: async () => ({
        bytes: [],
        width: 1,
        height: 1
      })
    })
  });
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Ähnliche Bilder" }));
  await app.user.click(within(dialog()).getByRole("button", {
    name: "Suche starten"
  }));
  await app.runJobs();
  await waitFor(() => expect(within(dialog()).getAllByRole("checkbox")).toHaveLength(150));
  await app.user.click(within(dialog()).getByRole("button", {
    name: "Weitere Gruppen laden"
  }));
  await waitFor(() => expect(within(dialog()).getAllByRole("checkbox")).toHaveLength(153));
  expect(pages).toHaveBeenCalledWith(50, 50);
  for (const checkbox of within(dialog()).getAllByRole("checkbox")) expect(checkbox).not.toBeChecked();
});

test.each(["cancelled", "failed"] as const)("similar %s partial results cannot be trashed; restart restores actions", async state => {
  let bad = true;
  let ready = false;
  const app = await setupApp({ left: root, wrap: api => ({ ...api,
    similarPage: async (...args) => { const p = await api.similarPage(...args); ready = p.ready; return p; },
    jobs: async () => (await api.jobs()).map(j => ({ ...j, state: bad && ready ? state : j.state })),
  }) });
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Ähnliche Bilder" }));
  await app.user.click(within(dialog()).getByRole("button", { name: "Suche starten" }));
  await app.runJobs();
  await waitFor(() => expect(within(dialog()).getByRole("alert")).toBeVisible());
  for (const box of within(dialog()).queryAllByRole("checkbox")) await app.user.click(box);
  expect(within(dialog()).getByRole("button", { name: "In den Papierkorb …" })).toBeDisabled();
  if (state === "cancelled") expect(dialog()).toHaveTextContent("Die Suche wurde abgebrochen.");
  bad = false;
  await app.user.click(within(dialog()).getByRole("button", { name: "Suche starten" }));
  await app.runJobs();
  await app.user.click((await within(dialog()).findAllByRole("checkbox"))[0]!);
  expect(within(dialog()).getByRole("button", { name: "In den Papierkorb …" })).toBeEnabled();
});

test("similar uses hash labels, selected counts, consistent sizes and an empty rescan message", async () => {
  let empty = false;
  const app = await setupApp({ left: root, wrap: api => ({ ...api,
    similarPage: async (...args) => {
      const p = await api.similarPage(...args);
      return empty && p.ready ? { ...p, groups: [], total: 0, images: 0, reclaimableBytes: 0 } : p;
    },
    similarTrash: async (...args) => { const r = await api.similarTrash(...args); empty = true; return r; },
  }) });
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Ähnliche Bilder" }));
  await app.user.click(within(dialog()).getByRole("button", { name: "Suche starten" }));
  await app.runJobs();
  const boxes = await within(dialog()).findAllByRole("checkbox");
  expect(dialog()).toHaveTextContent("Gleicher Bildhash");
  expect(dialog()).not.toHaveTextContent("identisch");
  await app.user.click(boxes[0]!);
  const selected = within(dialog()).getByText(/1 Datei ausgewählt,/).textContent!;
  await app.user.click(within(dialog()).getByRole("button", { name: "In den Papierkorb …" }));
  expect(screen.getByRole("alertdialog")).toHaveTextContent(selected);
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Ja, Papierkorb" }));
  await app.runJobs();
  await waitFor(() => expect(dialog()).toHaveTextContent("Keine ähnlichen Bilder mehr gefunden."));
});

test("English hash distance and selected text avoid identical and redundant claims", async () => {
  const app = await setupApp({ left: root, language: "en", wrap: api => ({ ...api,
    similarPage: async (...args) => { const p = await api.similarPage(...args); return { ...p, groups: p.groups.map(g => ({ ...g, entries: g.entries.map((e, i) => ({ ...e, distanceToReference: i })) })) }; },
  }) });
  await app.user.click(screen.getByRole("button", { name: "Tools" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Similar images" }));
  const d = screen.getByRole("dialog", { name: "Similar images" });
  await app.user.click(within(d).getByRole("button", { name: "Start search" }));
  await app.runJobs();
  const cards = await within(d).findAllByRole("article");
  expect(cards.some(c => c.textContent?.includes("Same image hash"))).toBe(true);
  expect(cards.every(c => !c.textContent?.includes("identical"))).toBe(true);
  expect(d).toHaveTextContent("Distance 1");
  await app.user.click(within(d).getAllByRole("checkbox")[0]!);
  const text = within(d).getByText(/1 file selected,/).textContent!;
  await app.user.click(within(d).getByRole("button", { name: "Move to trash …" }));
  expect(screen.getByRole("alertdialog")).toHaveTextContent(text);
  expect(screen.getByRole("alertdialog")).not.toHaveTextContent("redundant");
});
