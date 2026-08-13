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
  getQuickAccess,
  hidePaletteWindow,
  onFileIndexUpdated,
  onWindowFocused,
  searchFiles,
} from "../lib/bridge";
import { dedupeApps } from "../lib/search";
import type { AppEntry, FileEntry, PaletteItem, QuickAccessEntry } from "../lib/types";
import { useApp } from "./app";

export type { Section } from "../features/palette/sections";

interface PaletteCtx {
  query: string;
  setQuery: (q: string) => void;
  sections: Section[];
  flatItems: PaletteItem[];
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
  const historyPathRequest = useRef(0);
  const fileStatusKnown = useRef(false);
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
      searchFiles(searchText, 8)
        .then((response) => {
          if (request !== fileRequest.current) return;
          fileStatusKnown.current = true;
          indexStatusRef.current = { ready: response.ready, indexing: response.indexing };
          setFileResults(response.items);
          setFileResultQuery(normalized);
          setFileIndexReady(response.ready);
          setFileIndexing(response.indexing);
          setFilePathBrowse(response.pathBrowse);
          setFilesError(!response.pathBrowse && !response.ready && !response.indexing);
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
        setApps(list);
        setAppsError(false);
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

  // Ids already asked for this session. Apps without icons never appear in
  // the response map; remembering the request prevents a per-keystroke IPC
  // for every icon-less pinned app.
  const iconRequested = useRef<Set<string>>(new Set());

  useEffect(() => {
    const missing = iconRequestIds.filter((id) => !(id in appIcons) && !iconRequested.current.has(id));
    if (missing.length === 0) return;
    for (const id of missing) iconRequested.current.add(id);
    // No cancellation guard: an effect re-run (new array identity) would
    // otherwise drop the in-flight result, and the requested set then
    // prevents a retry, leaving rows on monograms forever. Merging the
    // response whenever it lands is always safe - ids are stable keys.
    getAppIcons(missing)
      .then((icons) => {
        if (Object.keys(icons).length === 0) return;
        setAppIcons((previous) => ({ ...previous, ...icons }));
      })
      .catch(() => {});
  }, [iconRequestIds, appIcons]);

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
