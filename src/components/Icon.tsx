import { Terminal, Monitor, Server, ArrowLeft, ArrowRight, FileImage, FileArchive, FileText, FileCode, FileAudio, FileVideo, ChevronUp, ArrowUp, ArrowUpRight, ArrowLeftRight, ChevronDown, ChevronRight, Check, Columns2, Copy, EyeOff, File, FileInput, Folder, GitCompare, HardDrive, House, History, Images, Link2, List, PanelRight, Pencil, Pin, PlugZap, Plus, RefreshCw, ReplaceAll, Search, Settings, Unplug, Wrench, X, type LucideIcon } from "lucide-react";
import type { Entry } from "../ipc/types";

const icons = { terminal: Terminal, computer: Monitor, server: Server, back: ArrowLeft, forward: ArrowRight, sortUp: ChevronUp, image: FileImage, archive: FileArchive, text: FileText, code: FileCode, audio: FileAudio, video: FileVideo, up: ArrowUp, link: ArrowUpRight, swap: ArrowLeftRight, down: ChevronDown, right: ChevronRight, check: Check, columns: Columns2, copy: Copy, hidden: EyeOff, file: File, import: FileInput, dir: Folder, compare: GitCompare, disk: HardDrive, home: House, history: History, images: Images, couple: Link2, list: List, preview: PanelRight, edit: Pencil, pin: Pin, connect: PlugZap, plus: Plus, reload: RefreshCw, rename: ReplaceAll, search: Search, settings: Settings, disconnect: Unplug, tools: Wrench, broken: X, close: X } satisfies Record<string, LucideIcon>;
export type IconName = keyof typeof icons;
export function Icon({ name, size = 16, className = "" }: { name: IconName; size?: number; className?: string }) {
  const Component = icons[name];
  return <Component className={`ico ${className}`} size={size} strokeWidth={1.75} aria-hidden="true" focusable="false" />;
}
const fileIcons: [IconName, RegExp][] = [
  ["image", /\.(png|jpe?g|gif|webp|svg|bmp|avif|heic|tiff?)$/i],
  ["archive", /\.(zip|7z|rar|tar|gz|bz2|xz|zst)$/i],
  ["text", /\.(txt|md|rst|log|csv|pdf|odt|docx?)$/i],
  ["code", /\.(tsx?|jsx?|json|html?|css|rs|py|sh|c|cpp|h|go|java|toml|ya?ml|xml)$/i],
  ["audio", /\.(mp3|wav|flac|ogg|m4a|aac|opus)$/i],
  ["video", /\.(mp4|mkv|webm|avi|mov|m4v)$/i],
];

export function EntryIcon({ entry }: { entry: Entry }) {
  const kind = entry.name === ".." ? "up" : entry.kind === "symlink" ? (entry.brokenLink ? "broken" : "link") : entry.kind === "file" ? (fileIcons.find(([, pattern]) => pattern.test(entry.name))?.[0] ?? "file") : entry.kind;
  return <Icon name={kind} className={`ico-${kind}`} />;
}
