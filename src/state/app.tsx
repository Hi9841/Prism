import type { ReactNode } from "react";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import {
  getSystemTheme,
  loadState,
  onSystemThemeChange,
  onWinModeFailed,
  quitApp,
  saveState,
  setAlwaysOnTop,
  setShortcut,
  setTaskbarAlignment,
  setTaskbarScrollVolume,
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
  persistenceError: string | null;
  flushPersistence: () => Promise<void>;
  retryPersistence: () => Promise<void>;
  quit: () => Promise<void>;
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
  const [persistenceError, setPersistenceError] = useState<string | null>(null);

  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const historyRef = useRef(history);
  historyRef.current = history;
  const toastsRef = useRef(toasts);
  const persistTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const persistRequest = useRef<Promise<void> | null>(null);
  const stateLoadError = useRef<string | null>(null);
  const stateRead = useRef<Promise<void>>(Promise.resolve());
  const stateReadComplete = useRef(false);
  const settingsChangedAfterLoadError = useRef<Partial<Settings>>({});
  const historyChangedAfterLoadError = useRef(false);
  const persistRevision = useRef(0);
  const savedRevision = useRef(0);
  const widthFrame = useRef<number | null>(null);
  const renderedWidth = useRef<number | null>(null);
  const toastTimers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  // Initial load from disk: every value is sanitized against known sets so
  // a corrupt or hand-edited file degrades to defaults, never crashes.
  useEffect(() => {
    let active = true;
    stateRead.current = loadState()
      .then((state) => {
        if (!active) return;
        settingsRef.current = {
          ...sanitizeSettings(state?.settings),
          ...settingsChangedAfterLoadError.current,
        };
        settingsChangedAfterLoadError.current = {};
        setSettings(settingsRef.current);
        if (!historyChangedAfterLoadError.current) historyRef.current = sanitizeHistory(state?.history);
        historyChangedAfterLoadError.current = false;
        setHistory(historyRef.current);
      })
      .catch((error) => {
        if (!active) return;
        const message = `Could not load settings: ${String(error)}. Repair prism.json, then retry. Saving is paused to preserve your file.`;
        stateLoadError.current = message;
        setPersistenceError(message);
      })
      .finally(() => {
        if (active) {
          stateReadComplete.current = true;
          setReady(true);
        }
      });
    return () => {
      active = false;
    };
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

  // A flush drains changes made during a pending write before resolving.
  const flushPersistence = useCallback(async () => {
    if (persistTimer.current) clearTimeout(persistTimer.current);
    persistTimer.current = null;
    await stateRead.current;
    if (persistRevision.current === savedRevision.current) return;
    if (stateLoadError.current) throw new Error(stateLoadError.current);
    if (persistRequest.current) return persistRequest.current;
    const pending = (async () => {
      try {
        while (savedRevision.current !== persistRevision.current) {
          const revision = persistRevision.current;
          await saveState({ version: 3, settings: settingsRef.current, history: historyRef.current });
          savedRevision.current = revision;
        }
        setPersistenceError(null);
      } catch (error) {
        setPersistenceError(
          `Could not save settings: ${String(error)}. Free disk space or check folder permissions, then retry.`,
        );
        throw error;
      } finally {
        persistRequest.current = null;
      }
    })();
    persistRequest.current = pending;
    return pending;
  }, []);

  const schedulePersist = useCallback(() => {
    persistRevision.current += 1;
    if (persistTimer.current) clearTimeout(persistTimer.current);
    persistTimer.current = setTimeout(() => {
      void flushPersistence().catch(() => {});
    }, 350);
  }, [flushPersistence]);

  const retryPersistence = useCallback(async () => {
    if (stateLoadError.current) {
      try {
        const state = await loadState();
        settingsRef.current = {
          ...sanitizeSettings(state?.settings),
          ...settingsChangedAfterLoadError.current,
        };
        if (!historyChangedAfterLoadError.current) historyRef.current = sanitizeHistory(state?.history);
        setSettings(settingsRef.current);
        setHistory(historyRef.current);
        stateLoadError.current = null;
        settingsChangedAfterLoadError.current = {};
        historyChangedAfterLoadError.current = false;
        setPersistenceError(null);
      } catch (error) {
        const message = `Could not load settings: ${String(error)}. Repair prism.json, then retry. Saving is paused to preserve your file.`;
        stateLoadError.current = message;
        setPersistenceError(message);
        throw error;
      }
    }
    await flushPersistence();
  }, [flushPersistence]);

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
      if (!stateReadComplete.current || stateLoadError.current) {
        settingsChangedAfterLoadError.current = { ...settingsChangedAfterLoadError.current, ...patch };
      }
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
      if (event.isComposing) return;
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
      if (!stateReadComplete.current || stateLoadError.current) historyChangedAfterLoadError.current = true;
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
    if (!stateReadComplete.current || stateLoadError.current) historyChangedAfterLoadError.current = true;
    historyRef.current = [];
    setHistory([]);
    schedulePersist();
  }, [schedulePersist]);

  const removeHistory = useCallback(
    (id: string) => {
      const next = historyRef.current.filter((entry) => entry.id !== id);
      if (next.length === historyRef.current.length) return;
      if (!stateReadComplete.current || stateLoadError.current) historyChangedAfterLoadError.current = true;
      historyRef.current = next;
      setHistory(next);
      schedulePersist();
    },
    [schedulePersist],
  );

  const resetSettings = useCallback(async () => {
    await stateRead.current;
    if (stateLoadError.current) throw new Error(stateLoadError.current);
    // Apply native registrations first so a failed shortcut or taskbar
    // transition cannot leave the UI claiming that defaults are active.
    const previous = settingsRef.current;
    await setShortcut(DEFAULT_SETTINGS.shortcut);
    try {
      await setTaskbarAlignment(DEFAULT_SETTINGS.taskbarAlignment);
    } catch (error) {
      let alignmentRollbackError: unknown;
      try {
        // A rejected alignment operation may already have moved some HWNDs.
        await setTaskbarAlignment(previous.taskbarAlignment);
      } catch (rollbackError) {
        alignmentRollbackError = rollbackError;
      }
      try {
        await setShortcut(previous.shortcut);
      } catch (rollbackError) {
        // The successful registration is the last known native shortcut.
        updateSettings({ shortcut: DEFAULT_SETTINGS.shortcut });
        throw new Error(
          `Reset stopped: ${String(error)}. Shortcut restoration also failed: ${String(rollbackError)}. The shortcut remains ${DEFAULT_SETTINGS.shortcut}.${alignmentRollbackError ? ` Taskbar restoration failed: ${String(alignmentRollbackError)}.` : ""}`,
        );
      }
      if (alignmentRollbackError) {
        throw new Error(
          `Reset stopped: ${String(error)}. Could not restore taskbar alignment: ${String(alignmentRollbackError)}. Select the alignment again to retry.`,
        );
      }
      throw error;
    }
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
  }, [schedulePersist, updateSettings]);

  const dismissToast = useCallback((id: number) => {
    const toast = toastsRef.current.find((t) => t.id === id);
    if (!toast || toast.closing) return;
    const timer = toastTimers.current.get(id);
    if (timer) {
      clearTimeout(timer);
      toastTimers.current.delete(id);
    }
    // Mark the toast as closing so it plays its exit animation, then remove
    // it once the animation has finished.
    toastsRef.current = toastsRef.current.map((t) => (t.id === id ? { ...t, closing: true } : t));
    setToasts(toastsRef.current);
    const removeTimer = setTimeout(() => {
      toastTimers.current.delete(id);
      toastsRef.current = toastsRef.current.filter((t) => t.id !== id);
      setToasts(toastsRef.current);
    }, 160);
    toastTimers.current.set(id, removeTimer);
  }, []);

  const showToast = useCallback(
    (title: string, detail?: string, kind: Toast["kind"] = "success") => {
      const id = toastSeq++;
      // Publish synchronously so simultaneous failures share the same limit,
      // even before React commits the next render.
      toastsRef.current = [...toastsRef.current, { id, title, detail, kind }];
      setToasts(toastsRef.current);
      if (kind === "success") {
        const timer = setTimeout(() => dismissToast(id), 1900);
        toastTimers.current.set(id, timer);
      }
      // Keep at most two toasts on screen; evict the oldest with its exit
      // animation instead of dropping it instantly.
      const visible = toastsRef.current.filter((toast) => !toast.closing);
      for (const toast of visible.slice(0, -2)) dismissToast(toast.id);
    },
    [dismissToast],
  );

  const quit = useCallback(async () => {
    try {
      await flushPersistence();
      await quitApp();
    } catch (error) {
      showToast("Could not quit Prism", `Your changes are still open. ${String(error)}`, "error");
    }
  }, [flushPersistence, showToast]);

  useEffect(() => {
    if (persistenceError) {
      showToast("Settings need attention", "Open Settings to review the error and retry saving.", "error");
    }
  }, [persistenceError, showToast]);

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
      setWindowWidth(target).catch((error) => {
        showToast("Window width not applied", `Try selecting the width again. ${String(error)}`, "error");
      });
      return;
    }

    const startedAt = performance.now();
    const duration = 240;
    let lastDispatched = from;
    let lastDispatchTime = 0;
    const step = (now: number) => {
      const progress = Math.min(1, (now - startedAt) / duration);
      const eased = 1 - (1 - progress) ** 3;
      const next = Math.round(from + (target - from) * eased);
      renderedWidth.current = next;
      if (progress >= 1 || (now - lastDispatchTime >= 45 && next !== lastDispatched)) {
        lastDispatched = next;
        lastDispatchTime = now;
        setWindowWidth(next).catch((error) => {
          if (progress >= 1)
            showToast("Window width not applied", `Try selecting the width again. ${String(error)}`, "error");
        });
      }
      if (progress < 1) widthFrame.current = requestAnimationFrame(step);
    };
    widthFrame.current = requestAnimationFrame(step);
    return () => {
      if (widthFrame.current !== null) cancelAnimationFrame(widthFrame.current);
      widthFrame.current = null;
    };
  }, [ready, settings.width, showToast]);

  useEffect(() => {
    if (!ready) return;
    setAlwaysOnTop(settings.alwaysOnTop).catch((error) => {
      showToast("Always on top not applied", `Toggle the setting to retry. ${String(error)}`, "error");
    });
  }, [ready, settings.alwaysOnTop, showToast]);

  useEffect(() => {
    if (!ready) return;
    setViewZoom(settings.viewZoom).catch((error) => {
      showToast("Zoom not applied", `Choose a zoom level to retry. ${String(error)}`, "error");
    });
  }, [ready, settings.viewZoom, showToast]);

  useEffect(() => {
    if (!ready) return;
    setTaskbarScrollVolume(settings.taskbarScrollVolume ?? true).catch((error) => {
      showToast(
        "Taskbar volume control not applied",
        `Toggle the setting to retry. ${String(error)}`,
        "error",
      );
    });
  }, [ready, settings.taskbarScrollVolume, showToast]);

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
      persistenceError,
      flushPersistence,
      retryPersistence,
      quit,
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
      persistenceError,
      flushPersistence,
      retryPersistence,
      quit,
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
    taskbarScrollVolume:
      typeof src.taskbarScrollVolume === "boolean"
        ? src.taskbarScrollVolume
        : DEFAULT_SETTINGS.taskbarScrollVolume,
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
