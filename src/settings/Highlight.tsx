import type { ReactNode } from "react";
import { normalizeSearch } from "./catalog";

export function Highlight({ text, query }: { text: string; query: string }) {
  const needle = normalizeSearch(query.trim());
  if (!needle) return <>{text}</>;
  // Map normalized characters back to source offsets (ß expands to two characters).
  const offsets: number[] = [];
  let normalized = "";
  for (let i = 0; i < text.length; i++) {
    const part = normalizeSearch(text[i]!);
    normalized += part;
    for (let j = 0; j < part.length; j++) offsets.push(i);
  }
  const pieces: ReactNode[] = [];
  let cursor = 0, sourceCursor = 0;
  for (;;) {
    const index = normalized.indexOf(needle, cursor);
    if (index < 0) break;
    const start = offsets[index]!, end = offsets[index + needle.length - 1]! + 1;
    pieces.push(text.slice(sourceCursor, start), <mark key={index}>{text.slice(start, end)}</mark>);
    sourceCursor = end; cursor = index + needle.length;
  }
  return <>{pieces}{text.slice(sourceCursor)}</>;
}
