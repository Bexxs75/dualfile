import { act, screen, waitFor, within } from "@testing-library/react";
import { test, expect } from "vitest";
import { DOCS, setupApp } from "./test-utils";

test("Suche: Namenstreffer erscheinen im Dialog", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => {
    app.runtime.controller.openDialog({ kind: "search", w: 0 });
  });
  const dlg = screen.getByRole("dialog", { name: "Suchen" });
  await app.user.type(within(dlg).getByLabelText("Name"), "notizen*");
  await app.user.click(within(dlg).getByRole("button", { name: "Suchen" }));
  await app.runJobs();
  const hit = await within(dlg).findByRole("option", { name: /notizen/i }, { timeout: 3000 });
  expect(hit).toBeInTheDocument();
  // Treffer-Klick schließt den Dialog und navigiert in den Ordner.
  await app.user.click(hit);
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Suchen" })).toBeNull());
});
