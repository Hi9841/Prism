import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { AppEntry, FileSearchResponse, PersistedState, QuickAccessEntry } from "./types";

export type PowerAction = "lock" | "sleep" | "shutdown" | "restart";
export type TaskbarThickness = "compact" | "default" | "adaptive";
export type TaskbarCombineMode = "always" | "whenFull" | "never";
export type TaskbarStartIcon = "system" | "gem" | "diamond" | "custom";

interface CustomStartIcon {
  id: string;
  /** Base64 PNG preview (96 x 96). */
  preview: string;
}

export interface TaskbarSettings {
  thickness: TaskbarThickness;
  autoHide: boolean;
  combineButtons: TaskbarCombineMode;
  startIcon: TaskbarStartIcon;
  selectedCustomIcon: string | null;
  customStartIcons: CustomStartIcon[];
}

interface PalettePresentation {
  open: boolean;
  source: string;
  anchor: {
    startButton: { left: number; top: number; right: number; bottom: number } | null;
    clickPoint: { x: number; y: number } | null;
    taskbarEdge: string | null;
    monitor: { left: number; top: number; right: number; bottom: number } | null;
    workArea: { left: number; top: number; right: number; bottom: number } | null;
  } | null;
  generation: number;
}

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function getAppVersion(): Promise<string> {
  if (!inTauri) return Promise.resolve("");
  return getVersion();
}

/* ---------------- clipboard ---------------- */

export async function copyText(text: string): Promise<void> {
  if (inTauri) {
    await writeText(text);
  } else if (navigator.clipboard) {
    await navigator.clipboard.writeText(text);
  }
}

/* ---------------- window ---------------- */

export function presentPaletteWindow(): Promise<boolean> {
  if (!inTauri) return Promise.resolve(true);
  return invoke<boolean>("present_palette");
}

export function hidePaletteWindow(): Promise<void> {
  if (!inTauri) return Promise.resolve();
  return invoke("hide_palette");
}

export async function isWindowVisible(): Promise<boolean> {
  if (!inTauri) return false;
  return getCurrentWindow().isVisible();
}

export function setAlwaysOnTop(on: boolean): Promise<void> {
  if (!inTauri) return Promise.resolve();
  return getCurrentWindow().setAlwaysOnTop(on);
}

export function setViewZoom(percent: number): Promise<void> {
  const scaleFactor = percent / 100;
  if (!inTauri) {
    const root = document.documentElement;
    root.style.setProperty("zoom", String(scaleFactor));
    root.style.width = `${100 / scaleFactor}%`;
    root.style.height = `${100 / scaleFactor}%`;
    return Promise.resolve();
  }
  return getCurrentWebview().setZoom(scaleFactor);
}

/** Emitted by Rust with the authoritative native visibility state. */
export function onToggleRequest(cb: (request: PalettePresentation) => void): () => void {
  if (!inTauri) return () => {};
  const unlisten = listen<PalettePresentation>("prism-toggle", (event) => cb(event.payload));
  return () => {
    unlisten.then((f) => f());
  };
}

export function onWindowFocused(cb: (focused: boolean) => void): () => void {
  if (!inTauri) return () => {};
  const unlisten = getCurrentWindow().onFocusChanged((e) => cb(e.payload));
  return () => {
    unlisten.then((f) => f());
  };
}

/* ---------------- rust commands ---------------- */

export async function getApps(): Promise<AppEntry[]> {
  if (!inTauri) return [];
  // StrictMode double-mounts fire two overlapping get_apps calls; share one
  // in-flight request so the payload is only transferred once.
  if (!appsRequest) {
    appsRequest = invoke<AppEntry[]>("get_apps").finally(() => {
      appsRequest = null;
    });
  }
  return appsRequest;
}

let appsRequest: Promise<AppEntry[]> | null = null;

/** Batched lazy icon payload: only the rows actually rendered request icons. */
export async function getAppIcons(ids: string[]): Promise<Record<string, string>> {
  if (!inTauri) return {};
  return invoke<Record<string, string>>("get_app_icons", { ids });
}

export async function refreshApps(): Promise<AppEntry[]> {
  if (!inTauri) return [];
  return invoke<AppEntry[]>("refresh_apps");
}

export async function launchApp(appId: string): Promise<void> {
  if (!inTauri) return;
  await invoke("launch_app", { id: appId });
}

export async function launchAppAsAdmin(appId: string): Promise<void> {
  if (!inTauri) return;
  await invoke("launch_app_as_admin", { id: appId });
}

export async function openPath(path: string): Promise<void> {
  if (!inTauri) return;
  await invoke("open_path", { path });
}

export async function runPathAsAdmin(path: string): Promise<void> {
  if (!inTauri) return;
  await invoke("run_path_as_admin", { path });
}

export async function searchFiles(query: string, limit = 20): Promise<FileSearchResponse> {
  if (!inTauri) {
    const lower = query.toLowerCase();
    const sample = [
      {
        name: "Project brief.docx",
        path: "C:\\Users\\You\\Documents\\Project brief.docx",
        parent: "C:\\Users\\You\\Documents",
        isDirectory: false,
      },
      {
        name: "Product photos",
        path: "C:\\Users\\You\\Pictures\\Product photos",
        parent: "C:\\Users\\You\\Pictures",
        isDirectory: true,
      },
    ];
    return {
      items: sample
        .filter((entry) => `${entry.name} ${entry.path}`.toLowerCase().includes(lower))
        .slice(0, limit),
      ready: true,
      indexing: false,
      pathBrowse: false,
      volumes: [{ drive: "C:\\", state: "ready", indexedCount: sample.length }],
      totalIndexed: sample.length,
    };
  }
  return invoke<FileSearchResponse>("search_files", { query, limit });
}

export async function rebuildFileIndex(): Promise<void> {
  if (!inTauri) return;
  await invoke("rebuild_file_index");
}

/**
 * Thumbnails for many paths in one IPC round trip, returned in input order.
 * A result page is up to 20 image files; batching replaces a per-file
 * request storm (and its per-response re-renders) with a single exchange.
 */
export async function getFileThumbnails(paths: string[]): Promise<(string | null)[]> {
  if (!inTauri) return paths.map(() => null);
  const thumbnails = await invoke<(string | null)[]>("get_file_thumbnails", { paths });
  return paths.map((_, index) => thumbnails[index] ?? null);
}

export async function getQuickAccess(): Promise<QuickAccessEntry[]> {
  if (!inTauri) {
    return [
      { name: "Home", path: "C:\\Users\\You", kind: "home" },
      { name: "Desktop", path: "C:\\Users\\You\\Desktop", kind: "desktop" },
      { name: "Downloads", path: "C:\\Users\\You\\Downloads", kind: "downloads" },
      { name: "Documents", path: "C:\\Users\\You\\Documents", kind: "documents" },
      { name: "Pictures", path: "C:\\Users\\You\\Pictures", kind: "pictures" },
      { name: "Music", path: "C:\\Users\\You\\Music", kind: "music" },
      { name: "Videos", path: "C:\\Users\\You\\Videos", kind: "videos" },
    ];
  }
  return invoke<QuickAccessEntry[]>("get_quick_access");
}

export async function performPowerAction(action: PowerAction): Promise<void> {
  if (!inTauri) return;
  await invoke("perform_power_action", { action });
}

export async function existingPaths(paths: string[]): Promise<string[]> {
  if (!inTauri) return paths;
  return invoke<string[]>("existing_paths", { paths });
}

export function onFileIndexUpdated(cb: () => void): () => void {
  if (!inTauri) return () => {};
  const unlisten = listen("file-index-updated", cb);
  return () => {
    unlisten.then((stop) => stop());
  };
}

export async function setWindowStyle(theme: "light" | "dark"): Promise<void> {
  if (!inTauri) return;
  await invoke("set_window_style", { theme });
}

export async function setWindowWidth(width: number): Promise<void> {
  if (!inTauri) return;
  await invoke("set_window_width", { width });
}

export async function setTaskbarAlignment(alignment: "left" | "center" | "right"): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_alignment", { alignment });
}

export async function getTaskbarSettings(): Promise<TaskbarSettings> {
  if (!inTauri) {
    return {
      thickness: "default",
      autoHide: false,
      combineButtons: "always",
      startIcon: "system",
      selectedCustomIcon: null,
      customStartIcons: [],
    };
  }
  return invoke<TaskbarSettings>("get_taskbar_settings");
}

export async function setTaskbarThickness(value: TaskbarThickness): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_thickness", { value });
}

export async function setTaskbarAutoHide(enabled: boolean): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_auto_hide", { enabled });
}

export async function setTaskbarCombineButtons(value: TaskbarCombineMode): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_combine_buttons", { value });
}

export async function setTaskbarStartIcon(value: TaskbarStartIcon): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_start_icon", { value });
}

export async function setCustomStartIcon(png: Uint8Array): Promise<void> {
  if (!inTauri) return;
  // Base64 keeps a 2 MB icon at ~2.7 MB instead of the ~8+ MB JSON number
  // array produced by serializing raw bytes.
  await invoke("set_custom_start_icon", { base64Png: bytesToBase64(png) });
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunk) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(base64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64);
  const buffer = new ArrayBuffer(binary.length);
  const bytes = new Uint8Array(buffer);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export async function selectCustomStartIcon(id: string): Promise<void> {
  if (!inTauri) return;
  await invoke("select_custom_start_icon", { id });
}

export async function removeCustomStartIcon(id: string): Promise<void> {
  if (!inTauri) return;
  await invoke("remove_custom_start_icon", { id });
}

export async function getSystemTheme(): Promise<"light" | "dark"> {
  if (!inTauri) return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  return invoke<"light" | "dark">("get_system_theme");
}

/** Fired by the Rust side whenever the OS theme flips (system mode). */
export function onSystemThemeChange(cb: (theme: "light" | "dark") => void): () => void {
  if (!inTauri) return () => {};
  const unlisten = listen<{ theme: "light" | "dark" }>("system-theme-changed", (e) => cb(e.payload.theme));
  return () => {
    unlisten.then((f) => f());
  };
}

/** Fired when Win-key interception self-disables (e.g. replay rejected). */
export function onWinModeFailed(cb: (reason: string) => void): () => void {
  if (!inTauri) return () => {};
  const unlisten = listen<string>("win-mode-failed", (e) => cb(e.payload));
  return () => {
    unlisten.then((f) => f());
  };
}

export async function setShortcut(combo: string): Promise<void> {
  if (!inTauri) return;
  await invoke("set_shortcut", { combo });
}

export async function quitApp(): Promise<void> {
  if (!inTauri) return;
  await invoke("quit_app");
}

export async function loadState(): Promise<PersistedState | null> {
  if (!inTauri) return null;
  const raw = await invoke<unknown>("load_state");
  return (raw as PersistedState) ?? null;
}

export async function saveState(state: PersistedState): Promise<void> {
  if (!inTauri) return;
  await invoke("save_state", { state });
}
