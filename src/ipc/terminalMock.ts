import { CoreError, type DualfileApi, type TerminalOutput, type TerminalExit } from "./api";

export function createTerminalMock(): Pick<DualfileApi, "terminalOpen" | "terminalWrite" | "terminalResize" | "terminalClose" | "onTerminalOutput" | "onTerminalExit"> {
  let nextId = 0;
  const sessions = new Set<number>();
  const output = new Set<(event: TerminalOutput) => void>();
  const exits = new Set<(event: TerminalExit) => void>();
  const requireSession = (id: number) => { if (!sessions.has(id)) throw new CoreError("Unknown terminal session", "notFound"); };
  return {
    async terminalOpen() {
      if (sessions.size >= 8) throw new CoreError("At most 8 terminal sessions may be open", "invalidInput");
      const id = ++nextId;
      sessions.add(id);
      return id;
    },
    async terminalWrite(id, data) {
      requireSession(id);
      const bytes = new TextEncoder().encode(data);
      const encoded = btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join(""));
      output.forEach(handler => handler({ id, data: encoded }));
    },
    async terminalResize(id) { requireSession(id); },
    async terminalClose(id) {
      if (sessions.delete(id)) exits.forEach(handler => handler({ id, code: null }));
    },
    onTerminalOutput(handler) { output.add(handler); return () => { output.delete(handler); }; },
    onTerminalExit(handler) { exits.add(handler); return () => { exits.delete(handler); }; },
  };
}
