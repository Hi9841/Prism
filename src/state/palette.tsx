import type { ReactNode } from "react";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import {
  buildSections,
  historyFilePath,
  isClipboardKind,
  quickAccessPaletteItem,
  type Section,
} from "../features/palette/sections";
import {
  existingPaths,
  refreshApps as forceRefresh,
  getAppIcons,
  getApps,
  getFileThumbnail,
  getQuickAccess,
  hidePaletteWindow,
  onFileIndexUpdated,
  onWindowFocused,
  searchFiles,
} from "../lib/bridge";
import { appIconRetryDelay, selectAppIconRequestIds } from "../lib/iconLoading";
import { dedupeApps } from "../lib/search";
import type { AppEntry, FileEntry, PaletteItem, QuickAccessEntry } from "../lib/types";
import { useApp } from "./app";

export type { Section } from "../features/palette/sections";

interface PaletteCtx {
  query: string;
  setQuery: (q: string) => void;
  sections: Section[];
  flatItems: PaletteItem[];
  apps: AppEntry[];
  selected: number;
  move: (delta: number) => void;
  select: (index: number) => void;
  runSelected: () => void;
  runItem: (item: PaletteItem) => void;
  runItemAsAdmin: (item: PaletteItem) => void;
  appsLoaded: boolean;
  appsError: boolean;
  filesBusy: boolean;
  filesError: boolean;
  fileIndexing: boolean;
  pathBrowsing: boolean;
  refreshApps: () => void;
  reset: () => void;
}

const Ctx = createContext<PaletteCtx | null>(null);

export function PaletteProvider({ children }: { children: ReactNode }) {
  const app = useApp();
  const [query, setQueryState] = useState("");
  const [apps, setApps] = useState<AppEntry[]>([]);
  const [appsLoaded, setAppsLoaded] = useState(false);
  const [appsError, setAppsError] = useState(false);
  const [quickAccess, setQuickAccess] = useState<QuickAccessEntry[]>([]);
  const [fileResults, setFileResults] = useState<FileEntry[]>([]);
  const [fileResultQuery, setFileResultQuery] = useState("");
  const [filesSearching, setFilesSearching] = useState(false);
  const [filesError, setFilesError] = useState(false);
  const [fileIndexReady, setFileIndexReady] = useState(false);
  const [fileIndexing, setFileIndexing] = useState(true);
  const [filePathBrowse, setFilePathBrowse] = useState(false);
  const [fileIndexTick, setFileIndexTick] = useState(0);
  const [existingHistoryPaths, setExistingHistoryPaths] = useState<ReadonlySet<string>>(() => new Set());
  const [appIcons, setAppIcons] = useState<Readonly<Record<string, string>>>({});
  const [selected, setSelected] = useState(0);
  const fileRequest = useRef(0);
  const fileThumbnailCache = useRef<Map<string, string | null>>(new Map());
  const fileThumbnailInFlight = useRef<Set<string>>(new Set());
  const historyPathRequest = useRef(0);
  const fileStatusKnown = useRef(false);
  const iconSettled = useRef<Set<string>>(new Set());
  const iconInFlight = useRef<Set<string>>(new Set());
  const iconAttempts = useRef<Map<string, number>>(new Map());
  const iconRetryTimer = useRef<number | null>(null);
  const [iconRetryTick, setIconRetryTick] = useState(0);
  // Mirror of the index status for effects that must not re-run when the
  // state flips (adding the state to deps would double-fire searches).
  const indexStatusRef = useRef({ ready: false, indexing: true });

  const setQuery = useCallback((next: string) => {
    setQueryState(next);
    setSelected(0);
  }, []);

  useEffect(() => {
    let active = true;
    getApps()
      .then((list) => {
        if (!active) return;
        setApps(list);
        setAppsError(false);
      })
      .catch(() => {
        if (active) setAppsError(true);
      })
      .finally(() => {
        if (active) setAppsLoaded(true);
      });
    getQuickAccess()
      .then((items) => {
        if (active) setQuickAccess(items);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  const validateHistoryPaths = useCallback(() => {
    const request = ++historyPathRequest.current;
    const paths = app.history.flatMap((entry) => {
      const path = historyFilePath(entry.id);
      return path ? [path] : [];
    });

    // Keep the previous rows mounted while re-validating so the Recent
    // section doesn't blink out on every window focus; the fresh set replaces
    // them as soon as the existence check returns.
    if (paths.length === 0) {
      setExistingHistoryPaths(new Set());
      return;
    }
    existingPaths(paths)
      .then((existing) => {
        if (request === historyPathRequest.current) {
          setExistingHistoryPaths(new Set(existing));
        }
      })
      .catch(() => {});
  }, [app.history]);

  useEffect(() => {
    validateHistoryPaths();
  }, [validateHistoryPaths]);

  useEffect(
    () =>
      onFileIndexUpdated(() => {
        setFileIndexTick((tick) => tick + 1);
        // A refresh may have flipped ready/indexing; re-query the status.
        fileStatusKnown.current = false;
        validateHistoryPaths();
      }),
    [validateHistoryPaths],
  );

  useEffect(() => onWindowFocused((focused) => focused && validateHistoryPaths()), [validateHistoryPaths]);

  useEffect(() => {
    void fileIndexTick;
    const searchText = query.trim();
    const normalized = searchText.toLowerCase();
    const request = ++fileRequest.current;
    if (normalized.length < 2) {
      setFileResults([]);
      setFileResultQuery(normalized);
      setFilesSearching(false);
      setFilesError(false);
      setFilePathBrowse(false);
      // Status is already known and no index refresh happened since - skip
      // the IPC round trip on every backspace/reset.
      const status = indexStatusRef.current;
      if (fileStatusKnown.current && status.ready && !status.indexing) return;
      searchFiles("", 1)
        .then((response) => {
          if (request !== fileRequest.current) return;
          fileStatusKnown.current = true;
          indexStatusRef.current = { ready: response.ready, indexing: response.indexing };
          setFileIndexReady(response.ready);
          setFileIndexing(response.indexing);
        })
        .catch(() => {});
      return;
    }

    setFilesSearching(true);
    setFilesError(false);
    const timer = window.setTimeout(() => {
      searchFiles(searchText, 20)
        .then((response) => {
          if (request !== fileRequest.current) return;
          fileStatusKnown.current = true;
          indexStatusRef.current = { ready: response.ready, indexing: response.indexing };
          const itemsWithCachedThumbnails = response.items.map((entry) => {
            const thumbnail = fileThumbnailCache.current.get(entry.path);
            return thumbnail ? { ...entry, thumbnail } : entry;
          });
          setFileResults(itemsWithCachedThumbnails);
          setFileResultQuery(normalized);
          setFileIndexReady(response.ready);
          setFileIndexing(response.indexing);
          setFilePathBrowse(response.pathBrowse);
          setFilesError(!response.pathBrowse && !response.ready && !response.indexing);

          const candidates = itemsWithCachedThumbnails.filter((entry) => {
            if (entry.isDirectory || entry.thumbnail || fileThumbnailCache.current.has(entry.path))
              return false;
            const extension = entry.name.split(".").pop()?.toLowerCase();
            return (
              extension !== undefined && ["png", "jpg", "jpeg", "gif", "bmp", "webp"].includes(extension)
            );
          });
          const pending = candidates.filter((entry) => !fileThumbnailInFlight.current.has(entry.path));
          if (pending.length > 0) {
            for (const entry of pending) fileThumbnailInFlight.current.add(entry.path);
            void Promise.all(
              pending.map(async (entry) => {
                const thumbnail = await getFileThumbnail(entry.path).catch(() => null);
                fileThumbnailCache.current.set(entry.path, thumbnail);
                fileThumbnailInFlight.current.delete(entry.path);
                return [entry.path, thumbnail] as const;
              }),
            ).then((thumbnails) => {
              const byPath = new Map(thumbnails);
              setFileResults((current) =>
                current.map((entry) => {
                  if (entry.thumbnail) return entry;
                  const thumbnail = byPath.get(entry.path);
                  return thumbnail ? { ...entry, thumbnail } : entry;
                }),
              );
            });
          }
        })
        .catch(() => {
          if (request !== fileRequest.current) return;
          fileStatusKnown.current = true;
          indexStatusRef.current = { ready: false, indexing: false };
          setFileResults([]);
          setFileResultQuery(normalized);
          setFileIndexReady(false);
          setFileIndexing(false);
          setFilePathBrowse(false);
          setFilesError(true);
        })
        .finally(() => {
          if (request === fileRequest.current) setFilesSearching(false);
        });
    }, 35);
    return () => window.clearTimeout(timer);
  }, [query, fileIndexTick]);

  const refreshApps = useCallback(() => {
    setAppsLoaded(false);
    forceRefresh()
      .then((list) => {
        if (iconRetryTimer.current !== null) {
          window.clearTimeout(iconRetryTimer.current);
          iconRetryTimer.current = null;
        }
        iconSettled.current.clear();
        iconAttempts.current.clear();
        setApps(list);
        setAppsError(false);
        setIconRetryTick((tick) => tick + 1);
      })
      .catch(() => setAppsError(true))
      .finally(() => setAppsLoaded(true));
  }, []);

  const visibleApps = useMemo(() => dedupeApps(apps), [apps]);
  const pinnedApps = useMemo(() => {
    const appsById = new Map(visibleApps.map((entry) => [entry.appId, entry]));
    return app.settings.pinnedApps.flatMap((appId) => {
      const entry = appsById.get(appId);
      return entry ? [entry] : [];
    });
  }, [visibleApps, app.settings.pinnedApps]);
  const quickItems = useMemo(() => {
    const entriesByKind = new Map(quickAccess.map((entry) => [entry.kind, entry]));
    return app.settings.quickAccess.flatMap((kind) => {
      const entry = entriesByKind.get(kind);
      return entry ? [quickAccessPaletteItem(entry)] : [];
    });
  }, [quickAccess, app.settings.quickAccess]);
  const filesBusy = filesSearching || (!filePathBrowse && !fileIndexReady && fileIndexing);

  const { sections, flatItems } = useMemo(
    () =>
      buildSections({
        query,
        apps: visibleApps,
        pinnedApps,
        quickItems,
        quickAccessCollapsed: app.settings.quickAccessCollapsed,
        appGroups: app.settings.appGroups,
        sectionOrder: app.settings.sectionOrder,
        pinnedAppIds: app.settings.pinnedApps,
        history: app.history,
        existingHistoryPaths,
        appIcons,
        fileResults,
        fileResultQuery,
        filePathBrowse,
        filesBusy,
        filesError,
      }),
    [
      query,
      visibleApps,
      pinnedApps,
      quickItems,
      app.settings.quickAccessCollapsed,
      app.settings.appGroups,
      app.settings.sectionOrder,
      app.settings.pinnedApps,
      app.history,
      existingHistoryPaths,
      appIcons,
      fileResults,
      fileResultQuery,
      filePathBrowse,
      filesBusy,
      filesError,
    ],
  );

  // Lazily fill app icons for the rows that are actually rendered. The app
  // metadata list stays lean; one small batched IPC covers the visible set.
  const iconRequestIds = useMemo(() => {
    const ids = new Set<string>();
    for (const item of flatItems) {
      if (item.appId) ids.add(item.appId);
    }
    for (const appId of app.settings.pinnedApps) ids.add(appId);
    return [...ids];
  }, [flatItems, app.settings.pinnedApps]);

  useEffect(() => {
    void iconRetryTick;
    const missing = selectAppIconRequestIds(iconRequestIds, {
      appsLoaded,
      icons: appIcons,
      settled: iconSettled.current,
      inFlight: iconInFlight.current,
      attempts: iconAttempts.current,
    });
    if (missing.length === 0) return;
    for (const id of missing) {
      iconInFlight.current.add(id);
      iconAttempts.current.set(id, (iconAttempts.current.get(id) ?? 0) + 1);
    }

    getAppIcons(missing)
      .then((icons) => {
        // The app cache is ready at this point, so omitted ids are genuinely
        // iconless and should not issue another request on every keystroke.
        for (const id of missing) iconSettled.current.add(id);
        if (Object.keys(icons).length > 0) {
          setAppIcons((previous) => ({ ...previous, ...icons }));
        }
      })
      .catch(() => {
        const attempt = Math.max(...missing.map((id) => iconAttempts.current.get(id) ?? 1));
        const delay = appIconRetryDelay(attempt);
        if (delay !== null && iconRetryTimer.current === null) {
          iconRetryTimer.current = window.setTimeout(() => {
            iconRetryTimer.current = null;
            setIconRetryTick((tick) => tick + 1);
          }, delay);
        }
      })
      .finally(() => {
        for (const id of missing) iconInFlight.current.delete(id);
      });
  }, [iconRequestIds, appIcons, appsLoaded, iconRetryTick]);

  useEffect(
    () => () => {
      if (iconRetryTimer.current !== null) window.clearTimeout(iconRetryTimer.current);
    },
    [],
  );

  const move = useCallback(
    (delta: number) => {
      setSelected((previous) => {
        if (flatItems.length === 0) return 0;
        return Math.min(Math.max(previous + delta, 0), flatItems.length - 1);
      });
    },
    [flatItems.length],
  );

  useEffect(() => {
    setSelected((previous) => Math.min(previous, Math.max(0, flatItems.length - 1)));
  }, [flatItems.length]);

  const runItem = useCallback(
    async (item: PaletteItem) => {
      const clipboardItem = isClipboardKind(item.id);
      try {
        if (clipboardItem) {
          await item.run();
        } else {
          await Promise.all([hidePaletteWindow(), item.run()]);
        }
        app.pushHistory(item.id, item.historyTitle);
        if (clipboardItem) {
          app.showToast("Copied to clipboard", item.toastDetail ?? item.title);
        }
      } catch {
        app.showToast("Couldn’t open item", item.title);
      }
    },
    [app],
  );

  const runSelected = useCallback(() => {
    const item = flatItems[selected];
    if (item) void runItem(item);
  }, [flatItems, selected, runItem]);

  const runItemAsAdmin = useCallback(
    async (item: PaletteItem) => {
      if (!item.runAsAdmin) return;
      try {
        await hidePaletteWindow();
        await item.runAsAdmin();
        app.pushHistory(item.id, item.historyTitle);
      } catch (error) {
        app.showToast("Could not run as administrator", String(error));
      }
    },
    [app],
  );

  const reset = useCallback(() => {
    setQuery("");
    setSelected(0);
  }, [setQuery]);

  const value = useMemo<PaletteCtx>(
    () => ({
      query,
      setQuery,
      sections,
      flatItems,
      apps: visibleApps,
      selected,
      move,
      select: setSelected,
      runSelected,
      runItem,
      runItemAsAdmin,
      appsLoaded,
      appsError,
      filesBusy,
      filesError,
      fileIndexing,
      pathBrowsing: filePathBrowse,
      refreshApps,
      reset,
    }),
    [
      query,
      setQuery,
      sections,
      flatItems,
      visibleApps,
      selected,
      move,
      runSelected,
      runItem,
      runItemAsAdmin,
      appsLoaded,
      appsError,
      filesBusy,
      filesError,
      fileIndexing,
      filePathBrowse,
      refreshApps,
      reset,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function usePalette(): PaletteCtx {
  const context = useContext(Ctx);
  if (!context) throw new Error("usePalette outside PaletteProvider");
  return context;
}
