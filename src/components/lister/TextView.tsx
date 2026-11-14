import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useController, useLanguage, useT } from "../../app/context";
import { MAX_BLOCK_BYTES } from "../../ipc/types";
import { formatBytes } from "../../i18n";
import { FindBar, GotoBar } from "./FindBar";
import {
  LINE_SCAN_LIMIT,
  TEXT_PAGE_BYTES,
  countNewlines,
  decodeBytes,
  encodeNeedle,
  incompleteTail,
  lastLineEnd,
  leadingContinuation,
  nthLineEnd,
  parseTextGoto,
  percentOf,
  readRange,
  splitLines,
  type TextEncodingId,
} from "./lib";
import { handleScrollKey, handleWheel, type PagedNav } from "./scrollKeys";
import { useFileSearch } from "./useFileSearch";

export interface TextPosition {
  percent: number;
  /** Erste sichtbare Zeile (nur solange Zeilen gezählt werden), sonst null. */
  line: number | null;
}

interface TextViewProps {
  path: string;
  size: number;
  encoding: TextEncodingId;
  wrap: boolean;
  lineNumbers: boolean;
  fontSize: number;
  onPosition(position: TextPosition): void;
}

interface Page {
  /** Dateioffset des ersten und (exklusiv) letzten Bytes dieses Fensters. */
  start: number;
  end: number;
  bytes: Uint8Array;
  text: string;
  /** Nummer der Zeile vor der ersten Zeile des Fensters; null = Zeilen werden hier nicht gezählt. */
  baseLine: number | null;
}

interface Match {
  offset: number;
  chars: number;
  key: string;
}

type ScrollTarget = "top" | "bottom" | "match";

/** Textansicht: lädt ein Fenster von höchstens `TEXT_PAGE_BYTES`, am Rand wird die nächste Seite gelesen. */
export function TextView({ path, size, encoding, wrap, lineNumbers, fontSize, onPosition }: TextViewProps) {
  const { api, describeError } = useController();
  const tr = useT();
  const language = useLanguage();
  const search = useFileSearch(path);
  const [page, setPage] = useState<Page | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [match, setMatch] = useState<Match | null>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [matchCase, setMatchCase] = useState(false);
  const [gotoOpen, setGotoOpen] = useState(false);
  const [gotoError, setGotoError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const request = useRef(0);
  const lastKey = useRef<string | null>(null);
  const scrollTo = useRef<ScrollTarget | null>("top");
  const startRef = useRef(0);
  const lineMap = useRef(new Map<number, number>());
  const wide = encoding === "utf-16le" || encoding === "utf-16be";
  const lineHeight = Math.round(fontSize * 1.45);

  /** Nummer der Zeile vor `start` (zählt ab dem nächsten bekannten Stand) oder null jenseits der Zählgrenze. */
  const baseLineAt = useCallback(
    async (start: number): Promise<number | null> => {
      if (start === 0) return 0;
      if (start > LINE_SCAN_LIMIT) return null;
      const known = lineMap.current.get(start);
      if (known !== undefined) return known;
      let from = 0;
      let line = 0;
      for (const [offset, count] of lineMap.current) {
        if (offset <= start && offset >= from) {
          from = offset;
          line = count;
        }
      }
      while (from < start) {
        const raw = await readRange(api, path, from, Math.min(MAX_BLOCK_BYTES, start - from));
        if (raw.length === 0) break;
        line += countNewlines(raw, encoding);
        from += raw.length;
      }
      return line;
    },
    [api, path, encoding],
  );

  /** Lädt ein Fenster ab `start` (auf Zeichengrenzen und Zeilenenden zurechtgeschnitten). */
  const loadAt = useCallback(
    async (start: number, scroll: ScrollTarget, endLimit?: number): Promise<void> => {
      const id = ++request.current;
      try {
        let from = Math.max(0, Math.min(start, Math.max(0, size - 1)));
        if (wide) from -= from % 2;
        const want = Math.min(TEXT_PAGE_BYTES, size - from, endLimit === undefined ? Infinity : endLimit - from);
        const raw = await readRange(api, path, from, Math.max(0, want));
        if (id !== request.current) return;
        const atEof = from + raw.length >= size;
        const hitLimit = endLimit !== undefined && from + raw.length >= endLimit;
        const skip = from > 0 ? leadingContinuation(raw, encoding) : 0;
        let end = raw.length;
        if (!atEof && !hitLimit) {
          const cut = lastLineEnd(raw, encoding);
          end = cut > skip ? cut : raw.length - incompleteTail(raw, encoding);
        }
        const bytes = raw.subarray(skip, end);
        const effectiveStart = from + skip;
        const baseLine = await baseLineAt(effectiveStart);
        if (id !== request.current) return;
        if (baseLine !== null) {
          lineMap.current.set(effectiveStart, baseLine);
          lineMap.current.set(effectiveStart + bytes.length, baseLine + countNewlines(bytes, encoding));
        }
        scrollTo.current = scroll;
        startRef.current = effectiveStart;
        setPage({ start: effectiveStart, end: effectiveStart + bytes.length, bytes, text: decodeBytes(bytes, encoding, effectiveStart === 0), baseLine });
        setError(null);
      } catch (failure) {
        if (id === request.current) setError(describeError(failure, { path }));
      }
    },
    [api, path, size, encoding, wide, describeError, baseLineAt],
  );

  // Erstes Laden und Kodierungswechsel: an der bisherigen Stelle neu lesen.
  useEffect(() => {
    lineMap.current.clear();
    void loadAt(startRef.current, "top");
    return () => {
      request.current += 1;
    };
  }, [loadAt]);

  /** Anfang der Zeile, in der `offset` liegt (liest höchstens 64 KiB zurück). */
  const lineStartAt = useCallback(
    async (offset: number): Promise<number> => {
      const at = wide ? offset - (offset % 2) : offset;
      if (at <= 0) return 0;
      const from = Math.max(0, at - 64 * 1024);
      const raw = await readRange(api, path, from, at - from);
      const cut = lastLineEnd(raw, encoding);
      if (cut >= 0) return from + cut;
      return from === 0 ? 0 : at;
    },
    [api, path, encoding, wide],
  );

  const lines = useMemo(() => (page ? splitLines(page.text) : []), [page]);

  const hit = useMemo(() => {
    if (!match || !page || match.offset < page.start || match.offset >= page.end) return null;
    const before = decodeBytes(page.bytes.subarray(0, match.offset - page.start), encoding, page.start === 0).length;
    let index = 0;
    let lineStart = 0;
    const text = page.text;
    for (let at = text.indexOf("\n"); at >= 0 && at < before; at = text.indexOf("\n", at + 1)) {
      index += 1;
      lineStart = at + 1;
    }
    return { line: index, column: before - lineStart, displayColumn: Array.from(text.slice(lineStart, before)).length + 1, chars: match.chars };
  }, [match, page, encoding]);

  // Nach dem Laden bzw. bei einem neuen Treffer an die richtige Stelle scrollen.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el || !page) return;
    const target = scrollTo.current;
    scrollTo.current = null;
    if (target === "top") el.scrollTop = 0;
    else if (target === "bottom") el.scrollTop = el.scrollHeight;
    if (target === "match" || (target === null && hit)) el.querySelector(".hit")?.scrollIntoView?.({ block: "center" });
  }, [page, hit]);

  const reportPosition = useCallback(() => {
    const el = scroller.current;
    if (!el || !page) return;
    const span = Math.max(1, el.scrollHeight - el.clientHeight);
    const fraction = el.scrollHeight <= el.clientHeight ? 0 : el.scrollTop / span;
    const position = page.start + (page.end - page.start) * fraction;
    const line = page.baseLine === null ? null : page.baseLine + 1 + Math.floor(fraction * Math.max(0, lines.length - 1));
    onPosition({ percent: percentOf(position, size), line });
  }, [page, size, lines.length, onPosition]);
  useEffect(reportPosition, [reportPosition]);


  async function toPrevPage(): Promise<void> {
    if (!page || page.start === 0) return;
    const candidate = Math.max(0, page.start - TEXT_PAGE_BYTES);
    const aligned = wide ? candidate - (candidate % 2) : candidate;
    let start = aligned;
    if (aligned > 0) {
      const raw = await readRange(api, path, aligned, Math.min(MAX_BLOCK_BYTES, page.start - aligned));
      const first = nthLineEnd(raw, encoding, 1);
      start = first >= 0 ? aligned + first : aligned;
    }
    await loadAt(start, "bottom", page.start);
  }
  const nav: PagedNav = {
    hasPrev: !!page && page.start > 0,
    hasNext: !!page && page.end < size,
    prevPage: () => void toPrevPage(),
    nextPage: () => page && void loadAt(page.end, "top"),
    toStart: () => void loadAt(0, "top"),
    toEnd: () => void (async () => loadAt(await lineStartAt(Math.max(0, size - TEXT_PAGE_BYTES / 2)), "bottom"))(),
    lineHeight,
  };

  // ---------------------------------------------------------------- Springen

  async function jump(input: string): Promise<void> {
    const goto = parseTextGoto(input);
    if (!goto) return setGotoError(tr("lister.gotoInvalid"));
    setGotoError(null);
    if (goto.kind === "percent") {
      const offset = Math.min(Math.max(0, size - 1), Math.floor((size * goto.percent) / 100));
      await loadAt(await lineStartAt(offset), "top");
      return setGotoOpen(false);
    }
    // Zeile N beginnt hinter dem (N-1)-ten Zeilenumbruch; gezählt wird nur am Dateianfang.
    let remaining = goto.line - 1;
    let from = 0;
    let found = remaining === 0 ? 0 : -1;
    while (found < 0 && from < size) {
      if (from > LINE_SCAN_LIMIT) return setGotoError(tr("lister.gotoLineTooFar", { line: goto.line, limit: formatBytes(LINE_SCAN_LIMIT, language) }));
      const raw = await readRange(api, path, from, Math.min(MAX_BLOCK_BYTES, size - from));
      if (raw.length === 0) break;
      const end = nthLineEnd(raw, encoding, remaining);
      if (end >= 0) found = from + end;
      else {
        remaining -= countNewlines(raw, encoding);
        from += raw.length;
      }
    }
    if (found < 0 || found >= size) return setGotoError(tr("lister.gotoLineMissing", { line: goto.line }));
    lineMap.current.set(found, goto.line - 1);
    await loadAt(found, "top");
    setGotoOpen(false);
  }

  // ---------------------------------------------------------------- Suchen

  async function find(direction: "forward" | "backward"): Promise<void> {
    if (!page) return;
    const needle = encodeNeedle(query, encoding);
    if (!needle) return search.setMessage(tr("lister.needleInvalid"));
    const key = `${query}\u0000${matchCase}\u0000${encoding}`;
    const sameSearch = match?.key === key;
    const sameQuery = lastKey.current === key;
    lastKey.current = key;
    let start: number;
    if (search.pending && search.pending.direction === direction && sameQuery) start = search.pending.next;
    else if (direction === "forward") start = sameSearch && match ? match.offset + 1 : page.start;
    else start = sameSearch && match ? match.offset : page.end;
    const offset = await search.run(needle, start, direction, !matchCase);
    if (offset === null) return;
    search.setMessage(tr("lister.foundByte", { offset }));
    setMatch({ offset, chars: query.length, key });
    if (offset >= page.start && offset < page.end) scrollTo.current = "match";
    else await loadAt(await lineStartAt(offset), "match");
  }

  function openFind(): void {
    setGotoOpen(false);
    setFindOpen(true);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const mod = event.ctrlKey && !event.altKey && !event.metaKey;
    if (mod && event.key.toLowerCase() === "f") {
      event.preventDefault();
      event.stopPropagation();
      return openFind();
    }
    if (mod && event.key.toLowerCase() === "g") {
      event.preventDefault();
      event.stopPropagation();
      setFindOpen(false);
      return setGotoOpen(true);
    }
    if (event.key === "F3" && query !== "") {
      event.preventDefault();
      event.stopPropagation();
      return void find(event.shiftKey ? "backward" : "forward");
    }
    if (event.key === "Escape") {
      if (search.cancel()) {
        event.preventDefault();
        return event.stopPropagation();
      }
      if (findOpen || gotoOpen) {
        event.preventDefault();
        event.stopPropagation();
        setFindOpen(false);
        setGotoOpen(false);
        scroller.current?.focus();
      }
    }
  }

  function onScrollerKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const el = scroller.current;
    if (el && handleScrollKey(event, el, nav)) event.preventDefault();
  }

  return (
    <div className="lv" ref={root} onKeyDown={onKeyDown}>
      <div className="lv-tools">
        <button type="button" className="tb small" onClick={openFind}>
          {tr("lister.find")} <kbd>{tr("lister.findKey")}</kbd>
        </button>
        <button type="button" className="tb small" onClick={() => (setFindOpen(false), setGotoOpen(true))}>
          {tr("lister.goto")} <kbd>{tr("lister.gotoKey")}</kbd>
        </button>
      </div>
      {findOpen ? (
        <FindBar
          query={query}
          onQuery={setQuery}
          matchCase={matchCase}
          onMatchCase={setMatchCase}
          onNext={() => void find("forward")}
          onPrev={() => void find("backward")}
          onClose={() => setFindOpen(false)}
          searching={search.searching}
          message={match && hit && page?.baseLine !== null && page && search.message === tr("lister.foundByte", { offset: match.offset })
            ? tr("lister.foundPosition", { line: page.baseLine + hit.line + 1, column: hit.displayColumn })
            : search.message}
          canContinue={search.pending !== null}
        />
      ) : null}
      {gotoOpen ? <GotoBar placeholder={tr("lister.gotoText")} error={gotoError} onSubmit={(value) => void jump(value)} onClose={() => setGotoOpen(false)} /> : null}
      {error ? (
        <p className="pane-msg error" role="alert">
          {tr("lister.error", { reason: error })}
        </p>
      ) : null}
      <div
        ref={scroller}
        className={`lv-scroll lv-text${wrap ? " wrap" : ""}`}
        role="textbox"
        aria-readonly="true"
        aria-multiline="true"
        aria-label={tr("lister.text.label")}
        tabIndex={0}
        data-viewer-focus
        style={{ fontSize, lineHeight: `${lineHeight}px` }}
        onKeyDown={onScrollerKeyDown}
        onWheel={(event) => handleWheel(event, nav)}
        onScroll={reportPosition}
      >
        {page && lines.length === 0 && !error ? <p className="pane-msg">{tr("lister.empty")}</p> : null}
        {lines.map((line, index) => (
          <div key={index} className={hit?.line === index ? "lv-line hit" : "lv-line"}>
            {lineNumbers ? <span className="lv-no">{page?.baseLine === null || !page ? "" : page.baseLine + index + 1}</span> : null}
            <span className="lv-tx">
              {hit?.line === index ? (
                <>
                  {line.slice(0, hit.column)}
                  <mark>{line.slice(hit.column, hit.column + hit.chars)}</mark>
                  {line.slice(hit.column + hit.chars)}
                </>
              ) : (
                line
              )}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
