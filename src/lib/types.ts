import type { LucideIcon } from "lucide-react";

export type TileTint = "iris" | "azure" | "mint" | "amber" | "rose" | "slate";

export type PaletteIcon =
  | { kind: "tile"; icon: LucideIcon; tint: TileTint }
  | { kind: "emoji"; char: string }
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
  /** Display title used when persisting to history */
  historyTitle: string;
  /** Extra line shown in the toast after a clipboard-style run */
  toastDetail?: string;
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
   *  taskbar, appsFolder, registry, programs. */
  source?: string;
  /** Searchable aliases: exe names, folder names, publisher, description. */
  keywords?: string[];
}

export interface FileEntry {
  name: string;
  path: string;
  parent: string;
  isDirectory: boolean;
}

export interface QuickAccessEntry {
  name: string;
  path: string;
  kind: "home" | "desktop" | "downloads" | "documents" | "pictures" | "music" | "videos";
}

export interface FileSearchResponse {
  items: FileEntry[];
  ready: boolean;
  indexing: boolean;
  pathBrowse: boolean;
}

export type AccentId = "iris" | "azure" | "mint" | "amber" | "rose";
export type WindowEffect = "acrylic" | "mica" | "solid";
export type WindowWidth = 560 | 640 | 720;
export type ThemeMode = "system" | "dark" | "light";

export interface Settings {
  accent: AccentId;
  width: WindowWidth;
  effect: WindowEffect;
  shortcut: string;
  alwaysOnTop: boolean;
  theme: ThemeMode;
}

export const DEFAULT_SETTINGS: Settings = {
  accent: "iris",
  width: 640,
  effect: "solid",
  shortcut: "Ctrl+Alt+Space",
  alwaysOnTop: true,
  theme: "system",
};

export interface PersistedState {
  /** Schema version for the state file; written by the backend. */
  version: number;
  settings: Settings;
  history: HistoryEntry[];
}
