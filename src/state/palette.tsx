import {
  Calculator,
  ClipboardCopy,
  Download,
  File,
  FileImage,
  FileText,
  FileVideo,
  Folder,
  Home,
  Images,
  Monitor,
  Music,
} from "lucide-react";
import type { ReactNode } from "react";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import {
  copyText,
  existingPaths,
  refreshApps as forceRefresh,
  getApps,
  getQuickAccess,
  hidePaletteWindow,
  launchApp,
  launchAppAsAdmin,
  onFileIndexUpdated,
  onWindowFocused,
  openPath,
  runPathAsAdmin,
  searchFiles,
} from "../lib/bridge";
import { sortApps } from "../lib/emoji";
import { formatNumber, isMathLike, tryEvaluate } from "../lib/math";
import { dedupeApps, fuzzy, fuzzyApps } from "../lib/search";
import type {
  AppEntry,
  FileEntry,
  HistoryEntry,
  PaletteItem,
  QuickAccessEntry,
  TileTint,
} from "../lib/types";
import { isElevatablePath } from "../lib/types";
import { useApp } from "./app";

export interface Section {
  id: string;
  label: string;
  items: PaletteItem[];
}

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

function appPaletteItem(app: AppEntry): PaletteItem {
  return {
    id: `app::${app.appId}`,
    title: app.name,
    subtitle: "Application",
    icon: { kind: "app", name: app.name, icon: app.icon },
    historyTitle: app.name,
    appId: app.appId,
    run: () => launchApp(app.appId),
    runAsAdmin: isElevatablePath(app.path) ? () => launchAppAsAdmin(app.appId) : undefined,
  };
}

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
  const sortedApps = useMemo(() => sortApps(visibleApps), [visibleApps]);
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

  const { sections, flatItems } = useMemo(() => {
    const normalized = query.trim();
    const searchQuery = normalized.toLowerCase();
    const out: Section[] = [];

    if (normalized.length === 0) {
      const pinnedItems = pinnedApps.map(appPaletteItem);
      if (pinnedItems.length > 0) {
        out.push({ id: "pinned", label: "Pinned", items: pinnedItems });
      }
      const pinnedItemIds = new Set(pinnedItems.map((item) => item.id));
      const recentItems: PaletteItem[] = [];
      for (const entry of app.history) {
        const item = rehydrate(entry, visibleApps, existingHistoryPaths);
        if (item && pinnedItemIds.has(item.id)) continue;
        if (item) recentItems.push(item);
        if (recentItems.length >= 5) break;
      }
      if (recentItems.length > 0) out.push({ id: "recent", label: "Recent", items: recentItems });

      const recentIds = new Set(recentItems.map((item) => item.id));
      const availableQuick = quickItems.filter((item) => !recentIds.has(item.id)).slice(0, 6);
      if (availableQuick.length > 0) {
        out.push({ id: "quick", label: "Quick Access", items: availableQuick });
      }

      if (sortedApps.length > 0) {
        const availableApps = sortedApps
          .filter((entry) => !app.settings.pinnedApps.includes(entry.appId))
          .slice(0, 8)
          .map(appPaletteItem);
        if (availableApps.length > 0) {
          out.push({ id: "apps", label: "Apps", items: availableApps });
        }
      }
    } else {
      const math = isMathLike(normalized) ? tryEvaluate(normalized) : null;
      if (math) {
        out.push({ id: "calc", label: "Calculator", items: [calcItem(math.value)] });
      }

      const quickHits = fuzzy(quickItems, normalized, { limit: 3 });
      if (quickHits.length > 0) {
        out.push({ id: "quick", label: "Quick Access", items: quickHits.map((hit) => hit.item) });
      }

      const appHits = fuzzyApps(visibleApps, normalized, 6, { preDeduped: true });
      const fileItems =
        fileResultQuery === searchQuery && fileResults.length > 0 ? fileResults.map(filePaletteItem) : [];

      if (filePathBrowse && fileItems.length > 0) {
        out.push({ id: "files", label: "Folder Contents", items: fileItems });
      }
      if (appHits.length > 0) {
        out.push({ id: "apps", label: "Apps", items: appHits.map(appPaletteItem) });
      }
      if (!filePathBrowse && fileItems.length > 0) {
        out.push({ id: "files", label: "Files & Folders", items: fileItems });
      }

      if (out.length === 0 && !filesBusy && !filesError && !filePathBrowse) {
        out.push({ id: "fallback", label: "No Local Matches", items: [copyItem(normalized)] });
      }
    }

    return { sections: out, flatItems: out.flatMap((section) => section.items) };
  }, [
    query,
    app.history,
    existingHistoryPaths,
    visibleApps,
    sortedApps,
    pinnedApps,
    app.settings.pinnedApps,
    quickItems,
    fileResults,
    fileResultQuery,
    filePathBrowse,
    filesBusy,
    filesError,
  ]);

  const move = useCallback(
    (delta: number) => {
      setSelected((previous) => {
        if (flatItems.length === 0) return 0;
        return Math.min(Math.max(previous + delta, 0), flatItems.length - 1);
      });
    },
    [flatItems.length],
  );

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

function calcItem(value: number): PaletteItem {
  const formatted = formatNumber(value);
  return {
    id: `calc::${value}`,
    title: formatted,
    subtitle: "Copy result to clipboard",
    icon: { kind: "tile", icon: Calculator, tint: "iris" },
    historyTitle: `Calculate ${formatted}`,
    toastDetail: formatted,
    run: () => copyText(formatted),
  };
}

function isClipboardKind(id: string): boolean {
  return id.startsWith("calc::") || id.startsWith("copy::");
}

function copyItem(query: string): PaletteItem {
  return {
    id: `copy::${query}`,
    title: `Copy “${query}”`,
    subtitle: "No files, folders, or apps matched",
    icon: { kind: "tile", icon: ClipboardCopy, tint: "slate" },
    historyTitle: query,
    run: () => copyText(query),
  };
}

function quickAccessPaletteItem(entry: QuickAccessEntry): PaletteItem {
  const iconMap = {
    home: Home,
    desktop: Monitor,
    downloads: Download,
    documents: FileText,
    pictures: Images,
    music: Music,
    videos: FileVideo,
  };
  const tintMap: Record<QuickAccessEntry["kind"], TileTint> = {
    home: "iris",
    desktop: "azure",
    downloads: "mint",
    documents: "slate",
    pictures: "amber",
    music: "rose",
    videos: "rose",
  };
  return {
    id: pathItemId(entry.path, true),
    title: entry.name,
    subtitle: entry.path,
    keywords: ["open", "folder", "files", entry.kind, entry.path],
    icon: { kind: "tile", icon: iconMap[entry.kind], tint: tintMap[entry.kind] },
    historyTitle: entry.name,
    run: () => openPath(entry.path),
  };
}

function filePaletteItem(entry: FileEntry): PaletteItem {
  const { icon, tint } = fileAppearance(entry);
  return {
    id: pathItemId(entry.path, entry.isDirectory),
    title: entry.name,
    subtitle: entry.parent,
    keywords: [entry.path, entry.parent],
    icon: { kind: "tile", icon, tint },
    historyTitle: entry.name,
    run: () => openPath(entry.path),
    runAsAdmin:
      !entry.isDirectory && isElevatablePath(entry.path) ? () => runPathAsAdmin(entry.path) : undefined,
  };
}

function fileAppearance(entry: FileEntry) {
  if (entry.isDirectory) return { icon: Folder, tint: "azure" as const };
  const extension = entry.name.split(".").pop()?.toLowerCase() ?? "";
  if (["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg"].includes(extension)) {
    return { icon: FileImage, tint: "amber" as const };
  }
  if (["mp4", "mov", "mkv", "avi", "webm"].includes(extension)) {
    return { icon: FileVideo, tint: "rose" as const };
  }
  if (["mp3", "wav", "flac", "m4a", "aac", "ogg"].includes(extension)) {
    return { icon: Music, tint: "mint" as const };
  }
  if (["txt", "md", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv"].includes(extension)) {
    return { icon: FileText, tint: "slate" as const };
  }
  return { icon: File, tint: "slate" as const };
}

function pathItemId(path: string, isDirectory: boolean): string {
  return `file::${isDirectory ? "d" : "f"}::${path}`;
}

function historyFilePath(id: string): string | null {
  const directoryPrefix = "file::d::";
  const filePrefix = "file::f::";
  if (id.startsWith(directoryPrefix)) return id.slice(directoryPrefix.length);
  if (id.startsWith(filePrefix)) return id.slice(filePrefix.length);
  return null;
}

function rehydrate(
  history: HistoryEntry,
  apps: AppEntry[],
  existingHistoryPaths: ReadonlySet<string>,
): PaletteItem | null {
  if (history.id.startsWith("app::")) {
    const appId = history.id.slice(5);
    const entry = apps.find((candidate) => candidate.appId === appId);
    return entry ? appPaletteItem(entry) : null;
  }
  const directoryPrefix = "file::d::";
  const filePrefix = "file::f::";
  const isDirectory = history.id.startsWith(directoryPrefix);
  const prefix = isDirectory ? directoryPrefix : filePrefix;
  if (!history.id.startsWith(prefix)) return null;
  const path = history.id.slice(prefix.length);
  if (!existingHistoryPaths.has(path)) return null;
  const boundary = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return filePaletteItem({
    name: history.title,
    path,
    parent: boundary > 0 ? path.slice(0, boundary) : path,
    isDirectory,
  });
}
