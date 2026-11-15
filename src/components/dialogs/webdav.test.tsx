import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../../app/context";
import { createRuntime } from "../../app/runtime";
import { createMockApi } from "../../ipc/mock";
import { ConnectionProfileForm, newSshProfile } from "./ConnectionProfileForm";

afterEach(cleanup);

test("WebDAV form defaults to HTTPS/443, keeps the user optional, and stores the base path in share", async () => {
  const api = createMockApi();
  const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language: "en" }) });
  const saved = vi.fn();
  render(<RuntimeProvider runtime={runtime}><ConnectionProfileForm api={api} initial={newSshProfile("dav")} onSaved={saved} onCancel={() => {}} /></RuntimeProvider>);
  const user = userEvent.setup();
  await user.selectOptions(screen.getByLabelText("Protocol"), "webdav");
  // HTTPS on by default, port 443, no plaintext warning, user not required (left empty).
  expect(screen.getByLabelText("Port")).toHaveValue(443);
  expect(screen.getByLabelText("Use HTTPS")).toBeChecked();
  expect(screen.queryByLabelText(/unencrypted/i)).toBeNull();
  await user.type(screen.getByLabelText("Base path on the server"), "/dav");
  await user.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(saved).toHaveBeenCalledOnce());
  const entry = (await api.connectionsList())[0]!;
  expect(entry.profile).toMatchObject({ protocol: "webdav", port: 443, share: "/dav", tls: "explicit", user: "" });
});

test("Turning HTTPS off switches to port 80 and requires the explicit plaintext confirmation", async () => {
  const api = createMockApi();
  const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: s => ({ ...s, language: "en" }) });
  render(<RuntimeProvider runtime={runtime}><ConnectionProfileForm api={api} initial={newSshProfile("dav")} onSaved={() => {}} onCancel={() => {}} /></RuntimeProvider>);
  const user = userEvent.setup();
  await user.selectOptions(screen.getByLabelText("Protocol"), "webdav");
  await user.click(screen.getByLabelText("Use HTTPS"));
  // http now: port drops to 80 and the plaintext warning appears.
  expect(screen.getByLabelText("Port")).toHaveValue(80);
  expect(screen.getByLabelText("Use HTTPS")).not.toBeChecked();
  const plaintext = screen.getByLabelText(/unencrypted/i);
  expect(plaintext).toBeInTheDocument();
  expect(plaintext).not.toBeChecked();
});
