import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useController, useT } from "../../app/context";
import { FindBar, GotoBar } from "./FindBar";
import { HEX_PAGE_BYTES, HEX_ROW, hexRows, offsetDigits, parseHexBytes, parseOffset, percentOf, readRange } from "./lib";
import { handleScrollKey, handleWheel, type PagedNav } from "./scrollKeys";
import { useFileSearch } from "./useFileSearch";

interface HexViewProps {
  path: string;
  size: number;
  fontSize: number;
  onPosition(position: { percent: number; offset: number }): void;
}

interface Page {
  start: number;
  bytes: Uint8Array;
}

interface Match {
  offset: number;
  length: number;
  key: string;
}

type ScrollTarget = "top" | "bottom" | "match";

/** Hexansicht: 16 Bytes je Zeile (Offset, Hex, ASCII), Fenster von `HEX_PAGE_BYTES`. */
export function HexView({ path, size, fontSize, onPosition }: HexViewProps) {
  const { api, describeError } = useController();
  const tr = useT();
  const search = useFileSearch(path);
  const [page, setPage] = useState<Page | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [match, setMatch] = useState<Match | null>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [matchCase, setMatchCase] = useState(false);
  const [asBytes, setAsBytes] = useState(false);
  const [gotoOpen, setGotoOpen] = useState(false);
  const [gotoError, setGotoError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const request = useRef(0);
  const lastKey = useRef<string | null>(null);
  const scrollTo = useRef<ScrollTarget | null>("top");
  const digits = offsetDigits(size);
  const lineHeight = Math.round(fontSize * 1.5);

  const loadAt = useCallback(
    async (offset: number, scroll: ScrollTarget): Promise<void> => {
      const id = ++request.current;
      try {
        const start = Math.floor(Math.max(0, Math.min(offset, Math.max(0, size - 1))) / HEX_PAGE_BYTES) * HEX_PAGE_BYTES;
        const bytes = await readRange(api, path, start, Math.min(HEX_PAGE_BYTES, size - start));
        if (id !== request.current) return;
        scrollTo.current = scroll;
        setPage({ start, bytes });
        setError(null);
      } catch (failure) {
        if (id === request.current) setError(describeError(failure, { path }));
      }
    },
    [api, path, size, describeError],
  );
  useEffect(() => {
    void loadAt(0, "top");
    return () => {
      request.current += 1;
    };
  }, [loadAt]);

  const rows = useMemo(() => (page ? hexRows(page.bytes, page.start, digits) : []), [page, digits]);
  const hitRange = match && page && match.offset < page.start + page.bytes.length && match.offset + match.length > page.start ? match : null;

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el || !page) return;
    const target = scrollTo.current;
    scrollTo.current = null;
    if (target === "top") el.scrollTop = 0;
    else if (target === "bottom") el.scrollTop = el.scrollHeight;
    else if (target === "match") el.querySelector(".hit")?.scrollIntoView?.({ block: "center" });
  }, [page, hitRange]);

  const reportPosition = useCallback(() => {
    const el = scroller.current;
    if (!el || !page) return;
    const fraction = el.scrollHeight <= el.clientHeight ? 0 : el.scrollTop / Math.max(1, el.scrollHeight - el.clientHeight);
    const offset = page.start + Math.floor((fraction * page.bytes.length) / HEX_ROW) * HEX_ROW;
    onPosition({ percent: percentOf(offset, size), offset });
  }, [page, size, onPosition]);
  useEffect(reportPosition, [reportPosition]);


  const pageEnd = page ? page.start + page.bytes.length : 0;
  const nav: PagedNav = {
    hasPrev: !!page && page.start > 0,
    hasNext: !!page && pageEnd < size,
    prevPage: () => page && void loadAt(page.start - HEX_PAGE_BYTES, "bottom"),
    nextPage: () => page && void loadAt(pageEnd, "top"),
    toStart: () => void loadAt(0, "top"),
    toEnd: () => void loadAt(Math.max(0, size - 1), "bottom"),
    lineHeight,
  };

  function jump(input: string): void {
    const offset = parseOffset(input);
    if (offset === null || offset >= Math.max(1, size)) return setGotoError(tr("lister.gotoInvalid"));
    setGotoError(null);
    setGotoOpen(false);
    setMatch({ offset, length: 1, key: "goto" });
    void loadAt(offset, "match");
  }

  async function find(direction: "forward" | "backward"): Promise<void> {
    if (!page) return;
    let needle: Uint8Array | null;
    if (asBytes) {
      needle = parseHexBytes(query);
      if (!needle) return search.setMessage(tr("lister.bytesInvalid"));
    } else {
      needle = query === "" ? null : new TextEncoder().encode(query);
      if (!needle) return search.setMessage(tr("lister.needleInvalid"));
    }
    const key = `${query}\u0000${matchCase}\u0000${asBytes}`;
    const sameSearch = match?.key === key;
    const sameQuery = lastKey.current === key;
    lastKey.current = key;
    let start: number;
    if (search.pending && search.pending.direction === direction && sameQuery) start = search.pending.next;
    else if (direction === "forward") start = sameSearch && match ? match.offset + 1 : page.start;
    else start = sameSearch && match ? match.offset : pageEnd;
    // Bytefolgen sind exakt; die Groß-/Kleinschreibung gilt nur für Text.
    const offset = await search.run(needle, start, direction, !asBytes && !matchCase);
    if (offset === null) return;
    search.setMessage(tr("lister.found", { offset: `0x${offset.toString(16)}` }));
    setMatch({ offset, length: needle.length, key });
    if (offset >= page.start && offset < pageEnd) scrollTo.current = "match";
    else await loadAt(offset, "match");
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const mod = event.ctrlKey && !event.altKey && !event.metaKey;
    if (mod && event.key.toLowerCase() === "f") {
      event.preventDefault();
      event.stopPropagation();
      setGotoOpen(false);
      return setFindOpen(true);
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

  function renderRow(index: number) {
    const row = rows[index]!;
    const offset = page!.start + index * HEX_ROW;
    const bytes = page!.bytes.subarray(index * HEX_ROW, (index + 1) * HEX_ROW);
    const isHit = hitRange !== null && offset < hitRange.offset + hitRange.length && offset + HEX_ROW > hitRange.offset;
    if (!isHit) {
      return (
        <div key={offset} className="hx-row" data-offset={offset}>
          <span className="hx-off">{row.offset}</span>
          <span className="hx-hex">{row.hex}</span>
          <span className="hx-asc">{row.ascii}</span>
        </div>
      );
    }
    const inHit = (i: number): boolean => offset + i >= hitRange.offset && offset + i < hitRange.offset + hitRange.length;
    return (
      <div key={offset} className="hx-row" data-offset={offset}>
        <span className="hx-off">{row.offset}</span>
        <span className="hx-hex">
          {[...bytes].map((byte, i) => (
            <span key={i} className={inHit(i) ? "hit" : undefined}>
              {byte.toString(16).padStart(2, "0")}
              {i === 7 ? "  " : i < bytes.length - 1 ? " " : ""}
            </span>
          ))}
        </span>
        <span className="hx-asc">
          {row.ascii.split("").map((char, i) => (
            <span key={i} className={inHit(i) ? "hit" : undefined}>
              {char}
            </span>
          ))}
        </span>
      </div>
    );
  }

  return (
    <div className="lv" onKeyDown={onKeyDown}>
      <div className="lv-tools">
        <button type="button" className="tb small" onClick={() => (setGotoOpen(false), setFindOpen(true))}>
          {tr("lister.find")} <kbd>Strg+F</kbd>
        </button>
        <button type="button" className="tb small" onClick={() => (setFindOpen(false), setGotoOpen(true))}>
          {tr("lister.goto")} <kbd>Strg+G</kbd>
        </button>
      </div>
      {findOpen ? (
        <FindBar
          query={query}
          onQuery={setQuery}
          matchCase={matchCase}
          onMatchCase={setMatchCase}
          bytes={{ on: asBytes, onChange: setAsBytes }}
          onNext={() => void find("forward")}
          onPrev={() => void find("backward")}
          onClose={() => setFindOpen(false)}
          searching={search.searching}
          message={search.message}
          canContinue={search.pending !== null}
        />
      ) : null}
      {gotoOpen ? <GotoBar placeholder={tr("lister.gotoHex")} error={gotoError} onSubmit={jump} onClose={() => setGotoOpen(false)} /> : null}
      {error ? (
        <p className="pane-msg error" role="alert">
          {tr("lister.error", { reason: error })}
        </p>
      ) : null}
      <div
        ref={scroller}
        className="lv-scroll lv-hex"
        role="log"
        aria-live="off"
        aria-label={tr("lister.hex.label")}
        tabIndex={0}
        data-viewer-focus
        style={{ fontSize, lineHeight: `${lineHeight}px` }}
        onKeyDown={(event) => {
          const el = scroller.current;
          if (el && handleScrollKey(event, el, nav)) event.preventDefault();
        }}
        onWheel={(event) => handleWheel(event, nav)}
        onScroll={reportPosition}
      >
        {page && rows.length === 0 && !error ? <p className="pane-msg">{tr("lister.empty")}</p> : null}
        {rows.map((_, index) => renderRow(index))}
      </div>
    </div>
  );
}
