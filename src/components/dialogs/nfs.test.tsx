import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../../app/context";
import { createRuntime } from "../../app/runtime";
import { createMockApi } from "../../ipc/mock";
import { ConnectionProfileForm, newSshProfile } from "./ConnectionProfileForm";

afterEach(cleanup);

test("NFS form saves export/uid/gid without user or password fields", async () => {
  const api = createMockApi();
  const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language: "en" }) });
  const saved = vi.fn();
  render(<RuntimeProvider runtime={runtime}><ConnectionProfileForm api={api} initial={newSshProfile("nas")} onSaved={saved} onCancel={() => {}} /></RuntimeProvider>);
  const user = userEvent.setup();
  await user.selectOptions(screen.getByLabelText("Protocol"), "nfs");
  expect(screen.getByLabelText("Port")).toHaveValue(2049);
  expect(screen.queryByLabelText("Transport")).toBeNull();
  expect(screen.queryByLabelText("User")).toBeNull();
  expect(screen.queryByLabelText(/password/i)).toBeNull();
  expect(screen.getByLabelText("User ID (uid)")).toHaveValue(1000);
  await user.type(screen.getByLabelText("Export"), "/srv/share");
  await user.clear(screen.getByLabelText("User ID (uid)"));
  await user.type(screen.getByLabelText("User ID (uid)"), "1234");
  await user.clear(screen.getByLabelText("Group ID (gid)"));
  await user.type(screen.getByLabelText("Group ID (gid)"), "4321");
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(saved).toHaveBeenCalledOnce());
  const entry = (await api.connectionsList())[0]!;
  expect(entry.profile).toMatchObject({ protocol: "nfs", port: 2049, export: "/srv/share", uid: 1234, gid: 4321 });
});
