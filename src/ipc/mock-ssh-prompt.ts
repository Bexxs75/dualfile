import type { SshPrompt } from "./types";
import type { MockScheduler } from "./mock";

/** Only the event/answer bridge; no dialog and no saved credentials. */
export function createMockSshPrompts(scheduler: MockScheduler) {
  const handlers = new Set<(prompt: SshPrompt) => void>();
  const pending = new Map<string, (text: string | null) => void>();
  let next = 0;
  let closed = false;
  return {
    onSshPrompt(handler: (prompt: SshPrompt) => void) {
      handlers.add(handler);
      return () => { handlers.delete(handler); };
    },
    async sshPromptAnswer(id: string, text: string | null): Promise<void> {
      const resolve = pending.get(id);
      if (!resolve) throw new Error("SSH-Abfrage abgelaufen");
      resolve(text);
    },
    requestSshPrompt(text: string): Promise<string | null> {
      if (closed || new TextEncoder().encode(text).length > 4096) return Promise.resolve(null);
      const lower = text.toLowerCase();
      const kind = lower.includes("are you sure you want to continue connecting") ? "hostKey"
        : lower.includes("passphrase for key") || lower.includes("enter passphrase") ? "passphrase"
          : lower.includes("password:") ? "password" : "other";
      const prompt: SshPrompt = {
        id: String(++next), kind, text: text.replace(/\p{Cc}/gu, ""), secret: kind !== "hostKey",
      };
      return new Promise((resolve) => {
        const timer = scheduler.setTimeout(() => finish(null), 120_000);
        function finish(answer: string | null) {
          scheduler.clearTimeout(timer);
          pending.delete(prompt.id);
          if (answer !== null && (new TextEncoder().encode(answer).length > 4096 || /[\r\n\0]/u.test(answer)
            || (kind === "hostKey" && answer !== "yes" && answer !== "no"))) answer = null;
          resolve(answer);
        }
        pending.set(prompt.id, finish);
        try { handlers.forEach((handler) => handler(prompt)); } catch { finish(null); }
      });
    },
    closeSshPrompts() {
      closed = true;
      pending.forEach((finish) => finish(null));
      handlers.clear();
    },
  };
}
