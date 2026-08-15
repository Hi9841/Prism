import type { LucideIcon } from "lucide-react";

export type TileTint = "iris" | "azure" | "mint" | "amber" | "rose" | "slate";

export type PaletteIcon =
  | { kind: "tile"; icon: LucideIcon; tint: TileTint }
  | { kind: "emoji"; char: string }
  | { kind: "image"; src: string; name: string }
  | { kind: "app"; name: string; icon?: string };

export interface PaletteItem {
  /** Stable unique id, e.g. "app::{appId}" or "file::d::{path}". */
  id: string;
  title: string;
  subtitle?: string;
  keywords?: string[];
  icon: PaletteIcon;
  /** Runs the selected result. */
  run: () => Promise<void> | void;
  /** Runs an eligible application or script through the Windows UAC prompt. */
  runAsAdmin?: () => Promise<void> | void;
  /** Display title used when persisting to history */
  historyTitle: string;
  /** Extra line shown in the toast after a clipboard-style run */
  toastDetail?: string;
  /** Stable app id when this result can be pinned. */
  appId?: string;
  /** Stable Quick Access key when this row can be reordered. */
  quickAccessKind?: QuickAccessKind;
}

export interface HistoryEntry {
  id: string;
  title: string;
  ts: number;
}

export interface AppEntry {
  name: string;
  appId: string;
  icon?: string;
  /** Lowercased, punctuation-stripped name for exact matching. */
  normalizedName?: string;
  /** Resolved launch target (exe path or URL). */
  path?: string;
  /** Launch arguments (shortcut arguments), when present. */
  args?: string;
  /** Working directory from the shortcut, when set. */
  workingDirectory?: string;
  /** Original shortcut location (.lnk/.url path), when present. */
  location?: string;
  /** AppUserModelID for packaged apps, when available. */
  aumid?: string;
  /** Where the entry came from: startMenu, desktop, documents, profile,
   *  taskbar, appsFolder, registry, appPaths, applications, programs. */
  source?: string;
  /** Searchable aliases: exe names, folder names, publisher, description. */
  keywords?: string[];
}

export interface AppGroup {
  id: string;
  name: string;
  appIds: string[];
  collapsed: boolean;
}

export interface FileEntry {
  name: string;
  path: string;
  parent: string;
  isDirectory: boolean;
  /** Small data URL preview for supported image files. */
  thumbnail?: string;
}

export const QUICK_ACCESS_KINDS = [
  "home",
  "desktop",
  "downloads",
  "documents",
  "pictures",
  "music",
  "videos",
] as const;
export type QuickAccessKind = (typeof QUICK_ACCESS_KINDS)[number];
export const DEFAULT_QUICK_ACCESS: QuickAccessKind[] = [
  "home",
  "desktop",
  "downloads",
  "documents",
  "pictures",
  "music",
];
export const QUICK_ACCESS_LIMIT = 6;
export const PINNED_APP_LIMIT = 64;
export const APP_GROUP_LIMIT = 16;
export const APP_GROUP_APP_LIMIT = 64;
export const DEFAULT_SECTION_ORDER = ["pinned", "recent", "quick", "apps"] as const;
export type BuiltInSectionId = (typeof DEFAULT_SECTION_ORDER)[number];
export const SECTION_ORDER_LIMIT = DEFAULT_SECTION_ORDER.length;

const ELEVATABLE_EXTENSIONS = new Set(["exe", "com", "bat", "cmd", "ps1", "vbs", "js", "wsf"]);

export function isElevatablePath(path: string | undefined): boolean {
  if (!path || /^[a-z][a-z0-9+.-]*:\/\//i.test(path)) return false;
  const filename = path.split(/[\\/]/).pop() ?? "";
  const boundary = filename.lastIndexOf(".");
  return boundary > 0 && ELEVATABLE_EXTENSIONS.has(filename.slice(boundary + 1).toLowerCase());
}

export function reorderPinnedApps(
  pinnedApps: readonly string[],
  sourceAppId: string,
  targetAppId: string,
): string[] {
  const sourceIndex = pinnedApps.indexOf(sourceAppId);
  const targetIndex = pinnedApps.indexOf(targetAppId);
  if (sourceIndex < 0 || targetIndex < 0 || sourceIndex === targetIndex) {
    return [...pinnedApps];
  }

  const reordered = [...pinnedApps];
  const [movedAppId] = reordered.splice(sourceIndex, 1);
  reordered.splice(targetIndex, 0, movedAppId);
  return reordered;
}

export function reorderQuickAccess(
  quickAccess: readonly QuickAccessKind[],
  sourceKind: QuickAccessKind,
  targetKind: QuickAccessKind,
): QuickAccessKind[] {
  const sourceIndex = quickAccess.indexOf(sourceKind);
  const targetIndex = quickAccess.indexOf(targetKind);
  if (sourceIndex < 0 || targetIndex < 0 || sourceIndex === targetIndex) {
    return [...quickAccess];
  }

  const reordered = [...quickAccess];
  const [movedKind] = reordered.splice(sourceIndex, 1);
  reordered.splice(targetIndex, 0, movedKind);
  return reordered;
}

export function reorderSections(
  sectionOrder: readonly string[],
  sourceId: string,
  targetId: string,
): string[] {
  const sourceIndex = sectionOrder.indexOf(sourceId);
  const targetIndex = sectionOrder.indexOf(targetId);
  if (sourceIndex < 0 || targetIndex < 0 || sourceIndex === targetIndex) {
    return [...sectionOrder];
  }

  const reordered = [...sectionOrder];
  const [movedId] = reordered.splice(sourceIndex, 1);
  reordered.splice(targetIndex, 0, movedId);
  return reordered;
}

export function reorderAppGroups(
  groups: readonly AppGroup[],
  sourceId: string,
  targetId: string,
): AppGroup[] {
  const sourceIndex = groups.findIndex((group) => group.id === sourceId);
  const targetIndex = groups.findIndex((group) => group.id === targetId);
  if (sourceIndex < 0 || targetIndex < 0 || sourceIndex === targetIndex) {
    return [...groups];
  }

  const reordered = [...groups];
  const [movedGroup] = reordered.splice(sourceIndex, 1);
  reordered.splice(targetIndex, 0, movedGroup);
  return reordered;
}

export interface QuickAccessEntry {
  name: string;
  path: string;
  kind: QuickAccessKind;
}

export type VolumeState = "ready" | "indexing" | "offline" | "error";

export interface VolumeCoverage {
  drive: string;
  state: VolumeState;
  indexedCount: number;
  totalProgress?: number;
}

export interface FileSearchResponse {
  items: FileEntry[];
  ready: boolean;
  indexing: boolean;
  pathBrowse: boolean;
  volumes: VolumeCoverage[];
  totalIndexed: number;
}

export type AccentId = "iris" | "azure" | "mint" | "amber" | "rose";
export type WindowEffect = "acrylic" | "mica" | "solid";
export type WindowWidth = 560 | 640 | 720;
export type ThemeMode = "system" | "dark" | "light";
export type TaskbarAlignment = "left" | "center" | "right";
export const VIEW_ZOOM_LEVELS = [70, 80, 90, 100, 110, 120, 130, 140, 150] as const;
export type ViewZoom = (typeof VIEW_ZOOM_LEVELS)[number];

export function stepViewZoom(current: ViewZoom, direction: -1 | 1): ViewZoom {
  const currentIndex = VIEW_ZOOM_LEVELS.indexOf(current);
  const nextIndex = Math.min(VIEW_ZOOM_LEVELS.length - 1, Math.max(0, currentIndex + direction));
  return VIEW_ZOOM_LEVELS[nextIndex];
}

export interface Settings {
  accent: AccentId;
  width: WindowWidth;
  viewZoom: ViewZoom;
  effect: WindowEffect;
  shortcut: string;
  alwaysOnTop: boolean;
  taskbarAlignment: TaskbarAlignment;
  theme: ThemeMode;
  quickAccess: QuickAccessKind[];
  quickAccessCollapsed: boolean;
  pinnedApps: string[];
  appGroups: AppGroup[];
  sectionOrder: string[];
}

export const DEFAULT_SETTINGS: Settings = {
  accent: "iris",
  width: 640,
  viewZoom: 100,
  effect: "solid",
  shortcut: "Win",
  alwaysOnTop: true,
  taskbarAlignment: "center",
  theme: "system",
  quickAccess: [...DEFAULT_QUICK_ACCESS],
  quickAccessCollapsed: false,
  pinnedApps: [],
  appGroups: [],
  sectionOrder: [...DEFAULT_SECTION_ORDER],
};

export interface PersistedState {
  /** Schema version for the state file; written by the backend. */
  version: number;
  settings: Settings;
  history: HistoryEntry[];
}
