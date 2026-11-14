import { useEffect, useRef, useState } from "react";
import { useT } from "../../app/context";
import type { DualfileApi } from "../../ipc/api";
import type { SshPrompt } from "../../ipc/types";
import { Modal } from "../Modal";

const clean = (text: string) => [...text.replace(/[\p{Cc}\p{Cf}]/gu, "")].slice(0, 4096).join("");

function PromptDialog({ prompt, answer }: { prompt: SshPrompt; answer: (text: string | null) => void }) {
  const tr = useT();
  const input = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    const element = input.current;
    return () => { if (element) element.value = ""; };
  }, []);
  function submit(text: string | null) {
    if (done.current) return;
    done.current = true;
    if (input.current) input.current.value = "";
    answer(text);
  }
  return <Modal title={tr(prompt.kind === "other" ? "ssh.prompt.unknown" : `ssh.prompt.${prompt.kind}`)} onClose={() => submit(null)} footer={<>
    <button onClick={() => submit(null)}>{tr("action.cancel")}</button>
    <button onClick={() => submit(prompt.kind === "hostKey" ? "yes" : input.current?.value ?? "")}>{tr("ssh.connect")}</button>
  </>}>
    <p style={{ overflowWrap: "anywhere", whiteSpace: "pre-wrap" }}>{clean(prompt.text)}</p>
    {prompt.kind !== "hostKey" && <form onSubmit={(event) => { event.preventDefault(); submit(input.current?.value ?? ""); }} autoComplete="off">
      <label>{tr("ssh.answer")}<input ref={input} data-autofocus type="password" autoComplete="off" maxLength={4096} /></label>
    </form>}
  </Modal>;
}

/** Answers exist only in the input and the one IPC call, never in app state or storage. */
export function SshPromptDialogs({ api }: { api: DualfileApi }) {
  const [queue, setQueue] = useState<SshPrompt[]>([]);
  const pending = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  useEffect(() => {
    const timers = pending.current;
    const unsubscribe = api.onSshPrompt((prompt) => {
      if (timers.has(prompt.id)) return;
      timers.set(prompt.id, setTimeout(() => {
        timers.delete(prompt.id);
        setQueue((items) => items.filter((item) => item.id !== prompt.id));
        void api.sshPromptAnswer(prompt.id, null).catch(() => {});
      }, 120_000));
      setQueue((items) => [...items, prompt]);
    });
    return () => {
      unsubscribe();
      timers.forEach((timer, id) => {
        clearTimeout(timer);
        void api.sshPromptAnswer(id, null).catch(() => {});
      });
      timers.clear();
    };
  }, [api]);
  const prompt = queue[0];
  if (!prompt) return null;
  return <PromptDialog key={prompt.id} prompt={prompt} answer={(text) => {
    clearTimeout(pending.current.get(prompt.id));
    pending.current.delete(prompt.id);
    setQueue((items) => items.filter((item) => item.id !== prompt.id));
    // Late answers can be rejected after the backend timeout. Never log the payload.
    void api.sshPromptAnswer(prompt.id, text).catch(() => {});
  }} />;
}
