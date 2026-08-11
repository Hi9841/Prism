import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { AppEntry, FileSearchResponse, PersistedState, QuickAccessEntry, WindowEffect } from "./types";

export type PowerAction = "lock" | "shutdown" | "restart";
export type TaskbarThickness = "compact" | "default" | "adaptive";
export type TaskbarCombineMode = "always" | "whenFull" | "never";
export type TaskbarStartIcon = "system" | "gem" | "diamond" | "custom";

export interface CustomStartIcon {
  id: string;
  preview: number[];
}

export interface TaskbarSettings {
  thickness: TaskbarThickness;
  autoHide: boolean;
  combineButtons: TaskbarCombineMode;
  showTaskView: boolean;
  showWidgets: boolean;
  startIcon: TaskbarStartIcon;
  selectedCustomIcon: string | null;
  customStartIcons: CustomStartIcon[];
}

export interface PalettePresentation {
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

export function centerPaletteWindow(): Promise<void> {
  if (!inTauri) return Promise.resolve();
  return getCurrentWindow().center();
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
  return invoke<AppEntry[]>("get_apps");
}

export async function refreshApps(): Promise<AppEntry[]> {
  if (!inTauri) return [];
  return invoke<AppEntry[]>("refresh_apps");
}

export async function launchApp(appId: string): Promise<void> {
  if (!inTauri) return;
  await invoke("launch_app", { id: appId });
}

export async function openPath(path: string): Promise<void> {
  if (!inTauri) return;
  await invoke("open_path", { path });
}

export async function searchFiles(query: string, limit = 8): Promise<FileSearchResponse> {
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
    };
  }
  return invoke<FileSearchResponse>("search_files", { query, limit });
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

export async function setWindowStyle(theme: "light" | "dark", effect: WindowEffect): Promise<void> {
  if (!inTauri) return;
  await invoke("set_window_style", { theme, effect });
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
      showTaskView: true,
      showWidgets: false,
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

export async function setTaskbarTaskView(visible: boolean): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_task_view", { visible });
}

export async function setTaskbarWidgets(visible: boolean): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_widgets", { visible });
}

export async function setTaskbarStartIcon(value: TaskbarStartIcon): Promise<void> {
  if (!inTauri) return;
  await invoke("set_taskbar_start_icon", { value });
}

export async function setCustomStartIcon(png: Uint8Array): Promise<void> {
  if (!inTauri) return;
  await invoke("set_custom_start_icon", { png: Array.from(png) });
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
