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
  setViewZoom,
  setWindowStyle,
  setWindowWidth,
} from "../lib/bridge";
import type {
  AccentId,
  HistoryEntry,
  QuickAccessKind,
  Settings,
  ThemeMode,
  WindowEffect,
  WindowWidth,
} from "../lib/types";
import {
  DEFAULT_QUICK_ACCESS,
  DEFAULT_SETTINGS,
  PINNED_APP_LIMIT,
  QUICK_ACCESS_KINDS,
  QUICK_ACCESS_LIMIT,
  stepViewZoom,
  VIEW_ZOOM_LEVELS,
} from "../lib/types";

export interface Toast {
  id: number;
  title: string;
  detail?: string;
}

interface AppCtx {
  ready: boolean;
  settings: Settings;
  updateSettings: (patch: Partial<Settings>) => void;
  openSettings: boolean;
  setOpenSettings: (open: boolean) => void;
  history: HistoryEntry[];
  pushHistory: (id: string, title: string) => void;
  clearHistory: () => void;
  toasts: Toast[];
  showToast: (title: string, detail?: string) => void;
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
  const persistTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const widthFrame = useRef<number | null>(null);
  const renderedWidth = useRef<number | null>(null);

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

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const showToast = useCallback(
    (title: string, detail?: string) => {
      const id = toastSeq++;
      setToasts((prev) => [...prev.slice(-2), { id, title, detail }]);
      setTimeout(() => dismissToast(id), 1900);
    },
    [dismissToast],
  );

  // Apply each native side effect only when its own setting changes.
  const effectiveTheme = settings.theme === "system" ? systemTheme : settings.theme;
  useEffect(() => {
    if (!ready) return;
    document.documentElement.dataset.accent = settings.accent;
    document.documentElement.dataset.surface = settings.effect === "solid" ? "solid" : "glass";
    document.documentElement.dataset.theme = effectiveTheme;
  }, [ready, settings.accent, settings.effect, effectiveTheme]);

  useEffect(() => {
    if (!ready) return;
    setWindowStyle(effectiveTheme, settings.effect).catch(() => {});
  }, [ready, settings.effect, effectiveTheme]);

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

  useEffect(() => {
    if (!ready) return;
    // The persisted shortcut must be applied at startup too, not just on
    // change. The command validates and registers before committing.
    setShortcut(settings.shortcut)
      .then(() => {})
      .catch((e: unknown) => {
        showToast("Shortcut unavailable", String(e));
      });
  }, [ready, settings.shortcut, showToast]);

  // Safe recovery: if Win-key interception self-disables (e.g. an elevated
  // app rejected the replay), drop back to the default shortcut so the UI
  // and reality stay in sync.
  useEffect(() => {
    if (!ready) return;
    const off = onWinModeFailed((reason) => {
      showToast("Win-key mode disabled", reason);
      updateSettings({ shortcut: DEFAULT_SETTINGS.shortcut });
    });
    return off;
  }, [ready, updateSettings, showToast]);

  const value = useMemo<AppCtx>(
    () => ({
      ready,
      settings,
      updateSettings,
      openSettings,
      setOpenSettings,
      history,
      pushHistory,
      clearHistory,
      toasts,
      showToast,
      dismissToast,
    }),
    [
      ready,
      settings,
      updateSettings,
      openSettings,
      history,
      pushHistory,
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
    effect: pick(src.effect, ["acrylic", "mica", "solid"], DEFAULT_SETTINGS.effect),
    shortcut: pick(
      src.shortcut,
      SHORTCUT_OPTIONS.map((o) => o.value),
      DEFAULT_SETTINGS.shortcut,
    ),
    alwaysOnTop: typeof src.alwaysOnTop === "boolean" ? src.alwaysOnTop : DEFAULT_SETTINGS.alwaysOnTop,
    taskbarAlignment: pick(src.taskbarAlignment, ["left", "center"], DEFAULT_SETTINGS.taskbarAlignment),
    theme: pick(src.theme, ["system", "dark", "light"], DEFAULT_SETTINGS.theme),
    quickAccess: sanitizeQuickAccess(src.quickAccess),
    pinnedApps: sanitizePinnedApps(src.pinnedApps),
  };
}

function sanitizePinnedApps(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of raw) {
    if (typeof value !== "string" || value.length === 0 || value.length > 4096 || seen.has(value)) {
      continue;
    }
    seen.add(value);
    result.push(value);
    if (result.length === PINNED_APP_LIMIT) break;
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

export type { AccentId, WindowEffect, WindowWidth };
