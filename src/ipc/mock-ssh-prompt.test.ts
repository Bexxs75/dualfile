import { describe, expect, test, vi } from "vitest";
import { createMockApi } from "./mock";

describe("SSH prompt bridge", () => {
  test("answers only the matching id once without storing the answer", async () => {
    const api = createMockApi();
    const prompts: unknown[] = [];
    api.onSshPrompt((p) => { prompts.push(p); void api.sshPromptAnswer(p.id, "marker-secret"); });
    expect(await api.requestSshPrompt("Password:\n")).toBe("marker-secret");
    expect(prompts).toEqual([{ id: "1", kind: "password", text: "Password:", secret: true }]);
    expect(JSON.stringify(prompts)).not.toContain("marker-secret");
    await expect(api.sshPromptAnswer("1", "late")).rejects.toThrow("abgelaufen");
    expect(await api.connectionsList()).toEqual([]);
  });
  test.each([
    ["passphrase for key", "passphrase", true],
    ["Enter passphrase", "passphrase", true],
    ["OTP:", "other", true],
    ["Are you sure you want to continue connecting", "hostKey", false],
  ] as const)("classifies %s and supports cancel", async (text, kind, secret) => {
    const api = createMockApi();
    api.onSshPrompt((p) => { expect(p.kind).toBe(kind); expect(p.secret).toBe(secret); void api.sshPromptAnswer(p.id, null); });
    expect(await api.requestSshPrompt(text)).toBeNull();
  });
  test.each(["yes", "no", "fingerprint", "yes\n"])("validates host-key answer %s", async (answer) => {
    const api = createMockApi();
    api.onSshPrompt((p) => { void api.sshPromptAnswer(p.id, answer); });
    expect(await api.requestSshPrompt("Are you sure you want to continue connecting")).toBe(
      answer === "yes" || answer === "no" ? answer : null,
    );
  });
  test("times out, rejects oversized requests, cancels on window close", async () => {
    vi.useFakeTimers();
    try {
      const api = createMockApi();
      const request = api.requestSshPrompt("Password:");
      vi.advanceTimersByTime(120_000);
      expect(await request).toBeNull();
      expect(await api.requestSshPrompt("x".repeat(4097))).toBeNull();
      const waiting = api.requestSshPrompt("Password:");
      api.closeSshPrompts();
      expect(await waiting).toBeNull();
      expect(await api.requestSshPrompt("Password:")).toBeNull();
      expect(vi.getTimerCount()).toBe(0);
    } finally { vi.useRealTimers(); }
  });
});

test("OpenSSH hosts import and connect without app pins or saved answers", async () => {
  const api = createMockApi();
  expect(await api.sshHostsList()).toEqual(["backup-box", "fileserver", "nas", "web-vps"]);
  await expect(api.sshHostResolve("-oProxyCommand=x")).rejects.toMatchObject({ kind: "invalidInput" });
  expect(await api.sshHostResolve("nas")).toMatchObject({ hostname: "nas.example", port: 22 });
  const { profile } = await api.connectionSave({
    id: "", name: "nas", protocol: "sftp", transport: "openssh", host: "nas", port: 0,
    user: "", auth: { kind: "password" }, tls: "none", saveSecret: false, allowPlaintext: false, startFolder: null,
  });
  await expect(api.connectionConnect(profile.id, "discard-marker")).resolves.toMatchObject({ state: "connected", secretStore: "none" });
  await expect(api.secretSet(profile.id, "discard-marker")).rejects.toMatchObject({ kind: "invalidInput" });
  await expect(api.connectionResetTrust(profile.id)).rejects.toMatchObject({ kind: "invalidInput" });
  expect((await api.connectionsList())[0]).toMatchObject({ pins: [], secretSet: false });
  expect(JSON.stringify(await api.loadSettings())).not.toContain("discard-marker");
});

test("changing transport closes the previous session", async () => {
  const api = createMockApi();
  const { profile } = await api.connectionSave({
    id: "", name: "nas", protocol: "sftp", transport: "openssh", host: "nas", port: 0,
    user: "", auth: { kind: "password" }, tls: "none", saveSecret: false, allowPlaintext: false, startFolder: null,
  });
  await api.connectionConnect(profile.id);
  expect((await api.connectionsList())[0]?.connected).toBe(true);
  const changed = await api.connectionSave({ ...profile, transport: "builtin", port: 22, user: "u" });
  expect(changed.connected).toBe(false);
});
