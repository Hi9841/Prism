import type { ReactNode } from "react";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import {
  getSystemTheme,
  loadState,
  onSystemThemeChange,
  onWinModeFailed,
  saveState,
  setAlwaysOnTop,
  setShortcut,
  setTaskbarAlignment,
  setViewZoom,
  setWindowStyle,
  setWindowWidth,
} from "../lib/bridge";
import type { AppGroup, HistoryEntry, QuickAccessKind, Settings, ThemeMode } from "../lib/types";
import {
  APP_GROUP_APP_LIMIT,
  APP_GROUP_LIMIT,
  DEFAULT_QUICK_ACCESS,
  DEFAULT_SECTION_ORDER,
  DEFAULT_SETTINGS,
  PINNED_APP_LIMIT,
  QUICK_ACCESS_KINDS,
  QUICK_ACCESS_LIMIT,
  SECTION_ORDER_LIMIT,
  stepViewZoom,
  VIEW_ZOOM_LEVELS,
} from "../lib/types";

interface Toast {
  id: number;
  title: string;
  detail?: string;
  kind: "success" | "error";
  /** Set while the toast animates out before removal. */
  closing?: boolean;
}

interface AppCtx {
  ready: boolean;
  settings: Settings;
  updateSettings: (patch: Partial<Settings>) => void;
  resetSettings: () => Promise<void>;
  openSettings: boolean;
  setOpenSettings: (open: boolean) => void;
  history: HistoryEntry[];
  pushHistory: (id: string, title: string) => void;
  removeHistory: (id: string) => void;
  clearHistory: () => void;
  toasts: Toast[];
  showToast: (title: string, detail?: string, kind?: Toast["kind"]) => void;
  dismissToast: (id: number) => void;
}

const Ctx = createContext<AppCtx | null>(null);

export const SHORTCUT_OPTIONS: { value: string; label: string }[] = [
  { value: "Win", label: "Win key" },
  { value: "Ctrl+Alt+Space", label: "Ctrl + Alt + Space" },
  { value: "Ctrl+Alt+S", label: "Ctrl + Alt + S" },
  { value: "Ctrl+Alt+Shift+S", label: "Ctrl + Alt + Shift + S" },
  { value: "Ctrl+Alt+P", label: "Ctrl + Alt + P" },
  { value: "Ctrl+Alt+Shift+P", label: "Ctrl + Alt + Shift + P" },
  { value: "Ctrl+Alt+Enter", label: "Ctrl + Alt + Enter" },
  { value: "Ctrl+Alt+Shift+Enter", label: "Ctrl + Alt + Shift + Enter" },
];

export const THEME_OPTIONS: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
];

const HISTORY_CAP = 20;

let toastSeq = 1;

export function AppProvider({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState(false);
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [openSettings, setOpenSettings] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [systemTheme, setSystemTheme] = useState<"light" | "dark">("dark");

  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const historyRef = useRef(history);
  historyRef.current = history;
  const toastsRef = useRef(toasts);
  toastsRef.current = toasts;
  const persistTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const widthFrame = useRef<number | null>(null);
  const renderedWidth = useRef<number | null>(null);
  const toastTimers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  // Initial load from disk: every value is sanitized against known sets so
  // a corrupt or hand-edited file degrades to defaults, never crashes.
  useEffect(() => {
    loadState()
      .then((state) => {
        if (state?.settings) setSettings(sanitizeSettings(state.settings));
        setHistory(sanitizeHistory(state?.history));
      })
      .catch(() => {})
      .finally(() => setReady(true));
  }, []);

  // Follow the OS theme live (only relevant in "system" mode).
  useEffect(() => {
    if (!ready) return;
    getSystemTheme()
      .then((t) => {
        setSystemTheme(t);
      })
      .catch(() => {});
    const off = onSystemThemeChange((t) => {
      setSystemTheme(t);
    });
    return off;
  }, [ready]);

  // Debounced full-state persistence - single writer, no races.
  const schedulePersist = useCallback(() => {
    if (persistTimer.current) clearTimeout(persistTimer.current);
    persistTimer.current = setTimeout(() => {
      saveState({
        version: 3,
        settings: settingsRef.current,
        history: historyRef.current,
      }).catch(() => {});
    }, 350);
  }, []);

  useEffect(
    () => () => {
      if (persistTimer.current) clearTimeout(persistTimer.current);
      for (const timer of toastTimers.current.values()) clearTimeout(timer);
      toastTimers.current.clear();
    },
    [],
  );

  const updateSettings = useCallback(
    (patch: Partial<Settings>) => {
      // Shortcut and taskbar alignment changes are applied by their pickers
      // before state is updated. Keeping native calls out of this generic
      // path prevents duplicate shell transitions.
      const next = { ...settingsRef.current, ...patch };
      const changed = (Object.keys(patch) as (keyof Settings)[]).some(
        (key) => !Object.is(settingsRef.current[key], next[key]),
      );
      if (!changed) return;
      settingsRef.current = next;
      setSettings(next);
      schedulePersist();
    },
    [schedulePersist],
  );

  const changeViewZoom = useCallback(
    (direction: -1 | 1) => {
      const current = settingsRef.current.viewZoom;
      const next = stepViewZoom(current, direction);
      if (next !== current) updateSettings({ viewZoom: next });
    },
    [updateSettings],
  );

  // Keep zoom shortcuts global so they work from both search and settings.
  // Capturing prevents the palette's Up/Down navigation from also firing.
  useEffect(() => {
    if (!ready) return;
    let wheelDelta = 0;
    let wheelResetTimer: ReturnType<typeof setTimeout> | null = null;

    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.metaKey) return;
      let direction: -1 | 1 | null = null;
      if (event.key === "ArrowUp" || event.key === "+" || event.key === "=") direction = 1;
      if (event.key === "ArrowDown" || event.key === "-" || event.key === "_") direction = -1;

      if (event.key === "0") {
        event.preventDefault();
        event.stopPropagation();
        updateSettings({ viewZoom: DEFAULT_SETTINGS.viewZoom });
      } else if (direction !== null) {
        event.preventDefault();
        event.stopPropagation();
        changeViewZoom(direction);
      }
    };

    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey || event.deltaY === 0) return;
      event.preventDefault();
      event.stopPropagation();

      const delta = event.deltaY * (event.deltaMode === WheelEvent.DOM_DELTA_PIXEL ? 1 : 16);
      if (wheelDelta !== 0 && Math.sign(delta) !== Math.sign(wheelDelta)) wheelDelta = 0;
      wheelDelta += delta;

      if (Math.abs(wheelDelta) >= 40) {
        changeViewZoom(wheelDelta < 0 ? 1 : -1);
        wheelDelta = 0;
      }
      if (wheelResetTimer) clearTimeout(wheelResetTimer);
      wheelResetTimer = setTimeout(() => {
        wheelDelta = 0;
      }, 180);
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("wheel", onWheel, { capture: true, passive: false });
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("wheel", onWheel, true);
      if (wheelResetTimer) clearTimeout(wheelResetTimer);
    };
  }, [ready, changeViewZoom, updateSettings]);

  const pushHistory = useCallback(
    (id: string, title: string) => {
      const next = [
        { id, title, ts: Date.now() },
        ...historyRef.current.filter((entry) => entry.id !== id),
      ].slice(0, HISTORY_CAP);
      historyRef.current = next;
      setHistory(next);
      schedulePersist();
    },
    [schedulePersist],
  );

  const clearHistory = useCallback(() => {
    historyRef.current = [];
    setHistory([]);
    schedulePersist();
  }, [schedulePersist]);

  const removeHistory = useCallback(
    (id: string) => {
      const next = historyRef.current.filter((entry) => entry.id !== id);
      if (next.length === historyRef.current.length) return;
      historyRef.current = next;
      setHistory(next);
      schedulePersist();
    },
    [schedulePersist],
  );

  const resetSettings = useCallback(async () => {
    // Apply native registrations first so a failed shortcut or taskbar
    // transition cannot leave the UI claiming that defaults are active.
    await setShortcut(DEFAULT_SETTINGS.shortcut);
    await setTaskbarAlignment(DEFAULT_SETTINGS.taskbarAlignment);
    const next: Settings = {
      ...DEFAULT_SETTINGS,
      quickAccess: [...DEFAULT_SETTINGS.quickAccess],
      pinnedApps: [],
      appGroups: [],
      sectionOrder: [...DEFAULT_SECTION_ORDER],
    };
    settingsRef.current = next;
    setSettings(next);
    schedulePersist();
  }, [schedulePersist]);

  const dismissToast = useCallback((id: number) => {
    const timer = toastTimers.current.get(id);
    if (timer) {
      clearTimeout(timer);
      toastTimers.current.delete(id);
    }
    const toast = toastsRef.current.find((t) => t.id === id);
    if (!toast || toast.closing) return;
    // Mark the toast as closing so it plays its exit animation, then remove
    // it once the animation has finished.
    setToasts((prev) => prev.map((t) => (t.id === id ? { ...t, closing: true } : t)));
    const removeTimer = setTimeout(() => {
      toastTimers.current.delete(id);
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 160);
    toastTimers.current.set(id, removeTimer);
  }, []);

  const showToast = useCallback(
    (title: string, detail?: string, kind: Toast["kind"] = "success") => {
      const id = toastSeq++;
      setToasts((prev) => [...prev, { id, title, detail, kind }]);
      if (kind === "success") {
        const timer = setTimeout(() => dismissToast(id), 1900);
        toastTimers.current.set(id, timer);
      }
      // Keep at most two toasts on screen; evict the oldest with its exit
      // animation instead of dropping it instantly.
      const visible = toastsRef.current;
      if (visible.length >= 2) dismissToast(visible[0].id);
    },
    [dismissToast],
  );

  // Apply each native side effect only when its own setting changes.
  const effectiveTheme = settings.theme === "system" ? systemTheme : settings.theme;
  useEffect(() => {
    if (!ready) return;
    document.documentElement.dataset.accent = settings.accent;
    document.documentElement.dataset.theme = effectiveTheme;
  }, [ready, settings.accent, effectiveTheme]);

  useEffect(() => {
    if (!ready) return;
    setWindowStyle(effectiveTheme).catch((error) => {
      showToast("Appearance not applied", String(error), "error");
    });
  }, [ready, effectiveTheme, showToast]);

  useEffect(() => {
    if (!ready) return;
    const target = settings.width;
    const from = renderedWidth.current;
    if (from === null || from === target || window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      renderedWidth.current = target;
      setWindowWidth(target).catch(() => {});
      return;
    }

    const startedAt = performance.now();
    const duration = 240;
    const step = (now: number) => {
      const progress = Math.min(1, (now - startedAt) / duration);
      const eased = 1 - (1 - progress) ** 3;
      const next = Math.round(from + (target - from) * eased);
      renderedWidth.current = next;
      setWindowWidth(next).catch(() => {});
      if (progress < 1) widthFrame.current = requestAnimationFrame(step);
    };
    widthFrame.current = requestAnimationFrame(step);
    return () => {
      if (widthFrame.current !== null) cancelAnimationFrame(widthFrame.current);
      widthFrame.current = null;
    };
  }, [ready, settings.width]);

  useEffect(() => {
    if (!ready) return;
    setAlwaysOnTop(settings.alwaysOnTop).catch(() => {});
  }, [ready, settings.alwaysOnTop]);

  useEffect(() => {
    if (!ready) return;
    setViewZoom(settings.viewZoom).catch(() => {});
  }, [ready, settings.viewZoom]);

  // Safe recovery: if Win-key interception self-disables (e.g. an elevated
  // app rejected the replay), drop back to the default shortcut so the UI
  // and reality stay in sync.
  useEffect(() => {
    if (!ready) return;
    const off = onWinModeFailed((reason) => {
      showToast("Win-key mode disabled", reason, "error");
      updateSettings({ shortcut: DEFAULT_SETTINGS.shortcut });
    });
    return off;
  }, [ready, updateSettings, showToast]);

  const value = useMemo<AppCtx>(
    () => ({
      ready,
      settings,
      updateSettings,
      resetSettings,
      openSettings,
      setOpenSettings,
      history,
      pushHistory,
      removeHistory,
      clearHistory,
      toasts,
      showToast,
      dismissToast,
    }),
    [
      ready,
      settings,
      updateSettings,
      resetSettings,
      openSettings,
      history,
      pushHistory,
      removeHistory,
      clearHistory,
      toasts,
      showToast,
      dismissToast,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppCtx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useApp outside AppProvider");
  return ctx;
}

/** Validates loaded settings against the known value sets; anything
 * unexpected falls back to the default. Mirrors the backend validator. */
export function sanitizeSettings(raw: unknown): Settings {
  const src = (raw ?? {}) as Record<string, unknown>;
  const pick = <T,>(value: unknown, allowed: readonly T[], fallback: T): T =>
    allowed.includes(value as T) ? (value as T) : fallback;
  return {
    accent: pick(src.accent, ["iris", "azure", "mint", "amber", "rose"], DEFAULT_SETTINGS.accent),
    width: pick(src.width, [560, 640, 720], DEFAULT_SETTINGS.width),
    viewZoom: pick(src.viewZoom, VIEW_ZOOM_LEVELS, DEFAULT_SETTINGS.viewZoom),
    shortcut: pick(
      src.shortcut,
      SHORTCUT_OPTIONS.map((o) => o.value),
      DEFAULT_SETTINGS.shortcut,
    ),
    alwaysOnTop: typeof src.alwaysOnTop === "boolean" ? src.alwaysOnTop : DEFAULT_SETTINGS.alwaysOnTop,
    taskbarAlignment: pick(
      src.taskbarAlignment,
      ["left", "center", "right"],
      DEFAULT_SETTINGS.taskbarAlignment,
    ),
    theme: pick(src.theme, ["system", "dark", "light"], DEFAULT_SETTINGS.theme),
    quickAccess: sanitizeQuickAccess(src.quickAccess),
    quickAccessCollapsed:
      typeof src.quickAccessCollapsed === "boolean"
        ? src.quickAccessCollapsed
        : DEFAULT_SETTINGS.quickAccessCollapsed,
    pinnedApps: sanitizePinnedApps(src.pinnedApps),
    appGroups: sanitizeAppGroups(src.appGroups),
    sectionOrder: sanitizeSectionOrder(src.sectionOrder),
  };
}

function sanitizeSectionOrder(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [...DEFAULT_SECTION_ORDER];
  const result: string[] = [];
  const seen = new Set<string>();
  for (const value of raw) {
    if (
      typeof value !== "string" ||
      !DEFAULT_SECTION_ORDER.includes(value as (typeof DEFAULT_SECTION_ORDER)[number]) ||
      seen.has(value)
    ) {
      continue;
    }
    seen.add(value);
    result.push(value);
    if (result.length >= SECTION_ORDER_LIMIT) break;
  }
  for (const id of DEFAULT_SECTION_ORDER) {
    if (!seen.has(id)) result.push(id);
  }
  return result;
}

function sanitizeAppGroups(raw: unknown): AppGroup[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const result: AppGroup[] = [];
  for (const value of raw) {
    if (!value || typeof value !== "object") continue;
    const src = value as Record<string, unknown>;
    const id = typeof src.id === "string" ? src.id.trim() : "";
    const name = typeof src.name === "string" ? src.name.trim() : "";
    if (!id || id.length > 96 || seen.has(id) || !name || name.length > 64) continue;
    const appIds = sanitizeIds(src.appIds, APP_GROUP_APP_LIMIT);
    result.push({
      id,
      name,
      appIds,
      collapsed: typeof src.collapsed === "boolean" ? src.collapsed : false,
    });
    seen.add(id);
    if (result.length >= APP_GROUP_LIMIT) break;
  }
  return result;
}

function sanitizePinnedApps(raw: unknown): string[] {
  return sanitizeIds(raw, PINNED_APP_LIMIT);
}

function sanitizeIds(raw: unknown, limit: number): string[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of raw) {
    if (typeof value !== "string" || value.length === 0 || value.length > 4096 || seen.has(value)) {
      continue;
    }
    seen.add(value);
    result.push(value);
    if (result.length === limit) break;
  }
  return result;
}

function sanitizeQuickAccess(raw: unknown): QuickAccessKind[] {
  if (!Array.isArray(raw)) return [...DEFAULT_QUICK_ACCESS];
  const seen = new Set<QuickAccessKind>();
  const result: QuickAccessKind[] = [];
  for (const value of raw) {
    if (!QUICK_ACCESS_KINDS.includes(value as QuickAccessKind) || seen.has(value as QuickAccessKind)) {
      continue;
    }
    const kind = value as QuickAccessKind;
    seen.add(kind);
    result.push(kind);
    if (result.length === QUICK_ACCESS_LIMIT) break;
  }
  return result;
}

export function sanitizeHistory(raw: unknown): HistoryEntry[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const history: HistoryEntry[] = [];
  for (const value of raw) {
    if (!value || typeof value !== "object") continue;
    const entry = value as Record<string, unknown>;
    if (
      typeof entry.id !== "string" ||
      entry.id.length === 0 ||
      entry.id.length > 4096 ||
      typeof entry.title !== "string" ||
      entry.title.length === 0 ||
      entry.title.length > 512 ||
      typeof entry.ts !== "number" ||
      !Number.isFinite(entry.ts) ||
      seen.has(entry.id)
    ) {
      continue;
    }
    seen.add(entry.id);
    history.push({ id: entry.id, title: entry.title, ts: entry.ts });
    if (history.length === HISTORY_CAP) break;
  }
  return history;
}
