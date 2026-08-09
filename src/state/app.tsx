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
  setWindowEffect,
  setWindowTheme,
  setWindowWidth,
} from "../lib/bridge";
import type { AccentId, HistoryEntry, Settings, ThemeMode, WindowEffect, WindowWidth } from "../lib/types";
import { DEFAULT_SETTINGS } from "../lib/types";

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
  { value: "Ctrl+Alt+Space", label: "Ctrl + Alt + Space" },
  { value: "Ctrl+Alt+S", label: "Ctrl + Alt + S" },
  { value: "Ctrl+Alt+Shift+S", label: "Ctrl + Alt + Shift + S" },
  { value: "Ctrl+Alt+P", label: "Ctrl + Alt + P" },
  { value: "Ctrl+Alt+Shift+P", label: "Ctrl + Alt + Shift + P" },
  { value: "Ctrl+Alt+Enter", label: "Ctrl + Alt + Enter" },
  { value: "Ctrl+Alt+Shift+Enter", label: "Ctrl + Alt + Shift + Enter" },
  { value: "Win", label: "Win key" },
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
        version: 2,
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
      // NOTE: shortcut changes are NOT applied here - the KeybindPicker
      // calls setShortcut directly and only updates state after the OS
      // registration succeeded. This avoids double-application and
      // persisting shortcuts that never activated.
      const next = { ...settingsRef.current, ...patch };
      settingsRef.current = next;
      setSettings(next);
      schedulePersist();
    },
    [schedulePersist],
  );

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
    setWindowTheme(effectiveTheme).catch(() => {});
    setWindowEffect(settings.effect).catch(() => {});
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
function sanitizeSettings(raw: unknown): Settings {
  const src = (raw ?? {}) as Record<string, unknown>;
  const pick = <T,>(value: unknown, allowed: readonly T[], fallback: T): T =>
    allowed.includes(value as T) ? (value as T) : fallback;
  return {
    accent: pick(src.accent, ["iris", "azure", "mint", "amber", "rose"], DEFAULT_SETTINGS.accent),
    width: pick(src.width, [560, 640, 720], DEFAULT_SETTINGS.width),
    effect: pick(src.effect, ["acrylic", "mica", "solid"], DEFAULT_SETTINGS.effect),
    shortcut: pick(
      src.shortcut,
      SHORTCUT_OPTIONS.map((o) => o.value),
      DEFAULT_SETTINGS.shortcut,
    ),
    alwaysOnTop: typeof src.alwaysOnTop === "boolean" ? src.alwaysOnTop : DEFAULT_SETTINGS.alwaysOnTop,
    theme: pick(src.theme, ["system", "dark", "light"], DEFAULT_SETTINGS.theme),
  };
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
