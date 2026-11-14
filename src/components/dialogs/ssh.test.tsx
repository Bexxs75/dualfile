import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { RuntimeProvider } from "../../app/context";
import { createRuntime } from "../../app/runtime";
import { createMockApi } from "../../ipc/mock";
import { SshPromptDialogs } from "./SshPromptDialog";
import { ConnectionProfileForm, newSshProfile } from "./ConnectionProfileForm";
import { ConnectionsDialog } from "./ConnectionsDialog";
import { CoreError } from "../../ipc/api";
import type { ReactNode } from "react";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
function setup(node: (api: ReturnType<typeof createMockApi>) => ReactNode) {
  const api = createMockApi();
  const runtime = createRuntime(api);
  runtime.store.dispatch({ type: "settings", update: (s) => ({ ...s, language: "de" }) });
  render(<RuntimeProvider runtime={runtime}>{node(api)}</RuntimeProvider>);
  return { api, user: userEvent.setup() };
}

test("transport switches hints, inherits empty values and resolves host selection", async () => {
  const saved = vi.fn();
  const { api, user } = setup((api) => <ConnectionProfileForm api={api} initial={newSshProfile("nas")} onSaved={saved} onCancel={() => {}} />);
  await screen.findByText(/OpenSSH nutzt/);
  await waitFor(() => expect(screen.getByLabelText("Host aus ssh-Konfiguration")).toHaveTextContent("backup-box"));
  await user.selectOptions(screen.getByLabelText("Host aus ssh-Konfiguration"), "backup-box");
  await screen.findByText(/Host backup-box.example/);
  expect(screen.getByLabelText("Server / ssh-Alias")).toHaveValue("backup-box");
  await user.selectOptions(screen.getByLabelText("Transport"), "builtin");
  expect(screen.getByText(/Eingebauter Transport: kein ssh-agent/)).toBeVisible();
  expect(screen.queryByText(/OpenSSH nutzt/)).toBeNull();
  await user.selectOptions(screen.getByLabelText("Transport"), "openssh");
  await user.click(screen.getByRole("button", { name: "Speichern" }));
  await waitFor(() => expect(saved).toHaveBeenCalledOnce());
  expect((await api.connectionsList())[0]?.profile).toMatchObject({ transport: "openssh", host: "backup-box", user: "", port: 0, saveSecret: false });
});

test("imports only selected hosts as OpenSSH profiles", async () => {
  const { api, user } = setup((api) => <ConnectionsDialog api={api} onClose={() => {}} />);
  await user.click(screen.getByRole("button", { name: "Aus ~/.ssh/config und known_hosts importieren…" }));
  await user.click(await screen.findByLabelText("nas"));
  await user.click(screen.getByLabelText("fileserver"));
  await user.click(screen.getByRole("button", { name: "Gewählte Hosts importieren" }));
  await waitFor(async () => expect(await api.connectionsList()).toHaveLength(2));
  const profiles = (await api.connectionsList()).map((e) => e.profile);
  expect(profiles.map((p) => p.host).sort()).toEqual(["fileserver", "nas"]);
  expect(profiles.every((p) => p.transport === "openssh" && !p.saveSecret && p.port === 0)).toBe(true);
});

test.each(["Password:", "Enter passphrase", "One-time token:"])("%s uses a password field, clears it and never logs the answer", async (text) => {
  const log = vi.spyOn(console, "log");
  const warn = vi.spyOn(console, "warn");
  const error = vi.spyOn(console, "error");
  const { api, user } = setup((api) => <SshPromptDialogs api={api} />);
  let pending!: Promise<string | null>;
  act(() => { pending = api.requestSshPrompt(text); });
  const input = screen.getByLabelText("Antwort");
  expect(input).toHaveAttribute("type", "password");
  expect(input).toHaveAttribute("autocomplete", "off");
  expect(input).toHaveFocus();
  await user.type(input, "never-save-marker");
  await user.click(screen.getByRole("button", { name: "Verbinden" }));
  expect(await pending).toBe("never-save-marker");
  expect(input).toHaveValue("");
  expect(screen.queryByLabelText("Antwort")).toBeNull();
  expect(document.body.innerHTML).not.toContain("never-save-marker");
  expect(JSON.stringify([log.mock.calls, warn.mock.calls, error.mock.calls, await api.connectionsList(), await api.loadSettings()])).not.toContain("never-save-marker");
});

test("host key is text, has no input, and answers yes; Escape answers null", async () => {
  const { api, user } = setup((api) => <SshPromptDialogs api={api} />);
  let pending!: Promise<string | null>;
  act(() => { pending = api.requestSshPrompt('Are you sure you want to continue connecting <img src=x onerror=alert(1)> SHA256:abc\u0001'); });
  expect(document.querySelector("img")).toBeNull();
  expect(screen.queryByLabelText("Antwort")).toBeNull();
  expect(screen.getByText(/<img src=x/)).toHaveTextContent("SHA256:abc");
  await user.click(screen.getByRole("button", { name: "Verbinden" }));
  expect(await pending).toBe("yes");
  act(() => { pending = api.requestSshPrompt("Are you sure you want to continue connecting"); });
  await user.keyboard("{Escape}");
  expect(await pending).toBeNull();
});

test("one prompt at a time in FIFO order, focus stays trapped, cancel clears input", async () => {
  const { api, user } = setup((api) => <SshPromptDialogs api={api} />);
  let first!: Promise<string | null>, second!: Promise<string | null>;
  act(() => { first = api.requestSshPrompt("First Password:"); second = api.requestSshPrompt("Second Password:"); });
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  expect(screen.queryByText("Second Password:")).toBeNull();
  for (let i = 0; i < 7; i++) { await user.tab(); expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement); }
  await user.type(screen.getByLabelText("Antwort"), "discard");
  await user.click(screen.getByRole("button", { name: "Abbrechen" }));
  expect(await first).toBeNull();
  expect(screen.getByText("Second Password:")).toBeVisible();
  expect(screen.getByLabelText("Antwort")).toHaveValue("");
  await user.keyboard("{Escape}");
  expect(await second).toBeNull();
});

test("OpenSSH profile import keeps the unsaved profile draft", async () => {
  const { api, user } = setup((api) => <ConnectionsDialog api={api} onClose={() => {}} />);
  await user.click(screen.getByRole("button", { name: "Neue Verbindung…" }));
  await user.type(screen.getByLabelText("Name"), "unsaved draft");
  await user.click(screen.getByRole("button", { name: "Aus ~/.ssh/config und known_hosts importieren…" }));
  await user.click(await screen.findByLabelText("backup-box"));
  await user.click(screen.getByRole("button", { name: "Gewählte Hosts importieren" }));
  await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue("unsaved draft"));
  expect((await api.connectionsList())[0]?.profile.host).toBe("backup-box");
});

test("profile deletion requires in-dialog confirmation and supports cancel", async () => {
  const { api, user } = setup((api) => {
    vi.spyOn(api, "connectionsList").mockResolvedValue([{ profile: { ...newSshProfile("example"), id: "p", name: "Example" }, connected: false, connectionId: null, root: null, secretSet: false, secretStore: "none", pins: [] }]);
    return <ConnectionsDialog api={api} onClose={() => {}} />;
  });
  const remove = vi.spyOn(api, "connectionDelete").mockResolvedValue();
  await user.click(await screen.findByRole("button", { name: "Löschen" }));
  expect(remove).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Abbrechen" }));
  expect(remove).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Löschen" }));
  await user.click(screen.getByRole("button", { name: "Löschen" }));
  await waitFor(() => expect(remove).toHaveBeenCalledWith("p"));
});

test("connection failure belongs to its profile, includes target and clears on editing", async () => {
  const { api, user } = setup((api) => {
    vi.spyOn(api, "connectionsList").mockResolvedValue([{ profile: { ...newSshProfile("example"), id: "p", name: "Example", port: 2222 }, connected: false, connectionId: null, root: null, secretSet: false, secretStore: "none", pins: [] }]);
    return <ConnectionsDialog api={api} onClose={() => {}} />;
  });
  vi.spyOn(api, "connectionConnect").mockRejectedValue(new CoreError("hidden", "unreachable"));
  const connect = await screen.findByRole("button", { name: "Verbinden" });
  await user.click(connect);
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("example.example:2222");
  expect(alert.textContent).not.toMatch(/\.:/);
  expect(alert).not.toHaveTextContent("hidden");
  expect(connect).toHaveClass("tb");
  expect(screen.getByRole("button", { name: "Bearbeiten" })).toHaveClass("tb");
  expect(alert.closest(".ssh-entry")).not.toBeNull();
  await user.click(screen.getByRole("button", { name: "Bearbeiten" }));
  expect(screen.queryByRole("alert")).toBeNull();
});

test("FTP has a server label and grouped checkbox; SSH resolution has a stable slot", async () => {
  const { user } = setup((api) => <ConnectionProfileForm api={api} initial={newSshProfile()} onSaved={() => {}} onCancel={() => {}} />);
  expect(document.querySelector(".ssh-resolved")).not.toBeNull();
  await user.selectOptions(screen.getByLabelText("Protokoll"), "ftp");
  expect(screen.getByLabelText("Server")).toBeVisible();
  expect(screen.getByLabelText("Passwort im Schlüsselbund speichern").closest("label")).toHaveClass("ssh-check");
});

test("retry clears the old connection error while the next attempt is pending", async () => {
  const { api, user } = setup((api) => {
    vi.spyOn(api, "connectionsList").mockResolvedValue([{ profile: { ...newSshProfile("example"), id: "p", name: "Example" }, connected: false, connectionId: null, root: null, secretSet: false, secretStore: "none", pins: [] }]);
    return <ConnectionsDialog api={api} onClose={() => {}} />;
  });
  const connect = vi.spyOn(api, "connectionConnect").mockRejectedValue(new CoreError("hidden", "auth"));
  await user.click(await screen.findByRole("button", { name: "Verbinden" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("example.example:22");
  connect.mockImplementation(() => new Promise(() => {}));
  await user.click(screen.getByRole("button", { name: "Verbinden" }));
  expect(screen.queryByRole("alert")).toBeNull();
});

test("failed profile deletion stays in confirmation and displays only translated errors", async () => {
  const { api, user } = setup((api) => {
    vi.spyOn(api, "connectionsList").mockResolvedValue([{ profile: { ...newSshProfile("example"), id: "p", name: "Example" }, connected: true, connectionId: "c", root: "sftp:c!/", secretSet: true, secretStore: "keychain", pins: [] }]);
    return <ConnectionsDialog api={api} onClose={() => {}} />;
  });
  vi.spyOn(api, "connectionDelete").mockRejectedValue(new CoreError("private keychain message", "secretRemovalFailed"));
  await user.click(await screen.findByRole("button", { name: "Löschen" }));
  await user.click(screen.getByRole("button", { name: "Löschen" }));
  expect(await screen.findByRole("alert")).not.toHaveTextContent("private keychain message");
  expect(screen.getByRole("button", { name: "Abbrechen" })).toBeEnabled();
  expect(screen.getByRole("dialog")).toHaveTextContent("Example");
});
