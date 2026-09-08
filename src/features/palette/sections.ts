/**
 * The palette's section policy: what results appear, grouped into which
 * sections, in what order, for an empty query and for a search query.
 *
 * Pure decision logic - no React, no state. The provider feeds it a
 * `PaletteSources` snapshot and renders whatever sections come back, so
 * every layout rule here is unit-testable through this single seam.
 */
import {
  Calculator,
  ClipboardCopy,
  Download,
  File,
  FileArchive,
  FileImage,
  FileText,
  FileVideo,
  Folder,
  Home,
  Images,
  Monitor,
  Music,
} from "lucide-react";
import {
  copyText,
  isPinnedToTaskbar,
  launchApp,
  launchAppAsAdmin,
  openPath,
  openPathLocation,
  runPathAsAdmin,
  setTaskbarPinned,
  showPathProperties,
  startFileDrag,
} from "../../lib/bridge";
import { sortApps } from "../../lib/emoji";
import { formatNumber, isMathLike, tryEvaluate } from "../../lib/math";
import { fuzzy, fuzzyApps } from "../../lib/search";
import type {
  AppEntry,
  AppGroup,
  FileEntry,
  HistoryEntry,
  PaletteItem,
  QuickAccessEntry,
  TileTint,
} from "../../lib/types";
import { isElevatablePath, isPicturePath, isTaskbarPinablePath } from "../../lib/types";
import { searchWindowsSettings } from "./windowsSettings";

export interface Section {
  id: string;
  label: string;
  items: PaletteItem[];
  collapsible?: boolean;
  collapsed?: boolean;
  groups?: AppGroupSection[];
}

interface AppGroupSection {
  groupId: string;
  id: string;
  label: string;
  items: PaletteItem[];
  collapsible: true;
  collapsed: boolean;
}

/** Everything the section policy may read, in one place. */
export interface PaletteSources {
  query: string;
  /** Deduplicated app entries (the provider dedupes before calling). */
  apps: AppEntry[];
  /** Resolved pinned entries, in user order. */
  pinnedApps: AppEntry[];
  /** Resolved quick-access items, in user order. */
  quickItems: PaletteItem[];
  quickAccessCollapsed: boolean;
  appGroups?: readonly AppGroup[];
  sectionOrder?: readonly string[];
  /** `settings.pinnedApps` - ids that must not repeat in the Apps section. */
  pinnedAppIds: readonly string[];
  history: HistoryEntry[];
  existingHistoryPaths: ReadonlySet<string>;
  fileThumbnails?: ReadonlyMap<string, string | null>;
  appIcons: Readonly<Record<string, string>>;
  fileResults: FileEntry[];
  fileResultQuery: string;
  filePathBrowse: boolean;
  filesBusy: boolean;
  filesError: boolean;
  fileIndexing?: boolean;
  fileIndexReady?: boolean;
}

/** Caps on the idle view; matching the displayed rows, not the data limits. */
const RECENT_LIMIT = 5;
const IDLE_QUICK_LIMIT = 6;
const IDLE_APPS_LIMIT = 8;
const SEARCH_QUICK_LIMIT = 3;
const SEARCH_APPS_LIMIT = 6;
const EMPTY_FILE_THUMBNAILS: ReadonlyMap<string, string | null> = new Map();

export function buildSections(sources: PaletteSources): {
  sections: Section[];
  flatItems: PaletteItem[];
} {
  const {
    query,
    apps,
    pinnedApps,
    quickItems,
    quickAccessCollapsed,
    appGroups = [],
    sectionOrder,
    pinnedAppIds,
    history,
    existingHistoryPaths,
    fileThumbnails = EMPTY_FILE_THUMBNAILS,
    appIcons,
    fileResults,
    fileResultQuery,
    filePathBrowse,
    filesBusy,
    filesError,
    fileIndexing,
    fileIndexReady,
  } = sources;
  const normalized = query.trim();
  const searchQuery = normalized.toLowerCase();
  const out: Section[] = [];

  if (normalized.length === 0) {
    // Only needed for the idle (empty query) view; sorting every app is
    // wasted work while the user is typing.
    const sortedApps = sortApps(apps);
    const pinnedItems = pinnedApps.map((entry) => appPaletteItem(entry, appIcons));
    if (pinnedItems.length > 0) {
      out.push({ id: "pinned", label: "Pinned", items: pinnedItems });
    }
    const pinnedItemIds = new Set(pinnedItems.map((item) => item.id));
    // One lookup map for the whole idle rebuild: history rehydration and the
    // pinned filter below both need membership checks per row.
    const appsById = new Map(apps.map((entry) => [entry.appId, entry]));
    const pinnedAppIdSet = new Set(pinnedAppIds);
    const recentItems: PaletteItem[] = [];
    for (const entry of history) {
      const item = rehydrate(entry, appsById, existingHistoryPaths, appIcons, fileThumbnails);
      if (item && pinnedItemIds.has(item.id)) continue;
      if (item) recentItems.push(item);
      if (recentItems.length >= RECENT_LIMIT) break;
    }
    if (recentItems.length > 0) {
      out.push({ id: "recent", label: "Recent", items: recentItems });
    }

    if (quickItems.length > 0) {
      const availableQuick = quickItems.slice(0, IDLE_QUICK_LIMIT);
      out.push({
        id: "quick",
        label: "Quick Access",
        items: quickAccessCollapsed ? [] : availableQuick,
        collapsible: true,
        collapsed: quickAccessCollapsed,
      });
    }

    const appsSection = buildAppsSection(
      sortedApps.filter((entry) => !pinnedAppIdSet.has(entry.appId)),
      appIcons,
      appGroups,
      IDLE_APPS_LIMIT,
    );
    if (appsSection) {
      out.push(appsSection);
    }
  } else {
    const math = isMathLike(normalized) ? tryEvaluate(normalized) : null;
    if (math) {
      out.push({ id: "calc", label: "Calculator", items: [calcItem(math.value)] });
    }

    const quickHits = fuzzy(quickItems, normalized, { limit: SEARCH_QUICK_LIMIT });
    if (quickHits.length > 0) {
      out.push({ id: "quick", label: "Quick Access", items: quickHits.map((hit) => hit.item) });
    }

    const appHits = fuzzyApps(apps, normalized, SEARCH_APPS_LIMIT, { preDeduped: true });
    const appPaths = new Set(
      appHits.map((app) => app.path?.toLowerCase()).filter((p): p is string => Boolean(p)),
    );

    const dedupedFiles = fileResults.filter((entry) => !appPaths.has(entry.path.toLowerCase()));
    const fileItems =
      fileResultQuery === searchQuery && dedupedFiles.length > 0 ? dedupedFiles.map(filePaletteItem) : [];

    if (filePathBrowse && fileItems.length > 0) {
      out.push({ id: "files", label: "Folder Contents", items: fileItems });
    }
    const settingsItems = filePathBrowse ? [] : searchWindowsSettings(normalized);
    const appsSection = buildAppsSection(appHits, appIcons, appGroups, SEARCH_APPS_LIMIT);
    if (appsSection) {
      out.push(appsSection);
    }
    if (settingsItems.length > 0) {
      out.push({ id: "settings", label: "Settings", items: settingsItems });
    }
    if (!filePathBrowse && fileItems.length > 0) {
      out.push({ id: "files", label: "Files & Folders", items: fileItems });
    }

    const indexingOrBusy = filesBusy || Boolean(fileIndexing) || fileIndexReady === false;
    if (out.length === 0 && !indexingOrBusy && !filesError && !filePathBrowse) {
      out.push({ id: "fallback", label: "No Local Matches", items: [copyItem(normalized)] });
    }
  }

  const sections = normalized.length === 0 ? orderSections(out, sectionOrder) : out;
  return {
    sections,
    flatItems: sections.flatMap((section) => [
      ...(section.groups?.flatMap((group) => group.items) ?? []),
      ...section.items,
    ]),
  };
}

function orderSections(sections: Section[], requestedOrder?: readonly string[]): Section[] {
  if (!requestedOrder || requestedOrder.length === 0) return sections;
  const byId = new Map(sections.map((section) => [section.id, section]));
  const ordered: Section[] = [];
  for (const id of requestedOrder) {
    const section = byId.get(id);
    if (!section) continue;
    ordered.push(section);
    byId.delete(id);
  }
  for (const section of sections) {
    if (byId.has(section.id)) ordered.push(section);
  }
  return ordered;
}

function buildAppsSection(
  entries: AppEntry[],
  icons: Readonly<Record<string, string>>,
  groups: readonly AppGroup[],
  limit: number,
): Section | null {
  if (entries.length === 0) return null;
  const entryById = new Map(entries.map((entry) => [entry.appId, entry]));
  const groupedIds = new Set<string>();
  const groupSections: AppGroupSection[] = [];

  for (const group of groups) {
    const items = group.appIds
      .filter((appId) => !groupedIds.has(appId))
      .map((appId) => entryById.get(appId))
      .filter((entry): entry is AppEntry => Boolean(entry))
      .slice(0, limit)
      .map((entry) => appPaletteItem(entry, icons));
    if (items.length === 0) continue;
    for (const item of items) {
      if (item.appId) groupedIds.add(item.appId);
    }
    groupSections.push({
      groupId: group.id,
      id: `apps-group-${group.id}`,
      label: group.name,
      items: group.collapsed ? [] : items,
      collapsible: true,
      collapsed: group.collapsed,
    });
  }

  const ungrouped = entries
    .filter((entry) => !groupedIds.has(entry.appId))
    .slice(0, limit)
    .map((entry) => appPaletteItem(entry, icons));
  if (ungrouped.length === 0 && groupSections.length === 0) return null;
  return { id: "apps", label: "Apps", items: ungrouped, groups: groupSections };
}

/** Runs the selected result (clipboard-style ids must not hide the window). */
export function isClipboardKind(id: string): boolean {
  return id.startsWith("calc::") || id.startsWith("copy::");
}

/** Queries the live taskbar pin state and flips it. The context menu labels
 *  itself from the same query, so the toggle re-checks at click time. */
async function toggleTaskbarPin(path: string): Promise<void> {
  const pinned = await isPinnedToTaskbar(path).catch(() => false);
  await setTaskbarPinned(path, !pinned);
}

function appPaletteItem(app: AppEntry, icons: Readonly<Record<string, string>>): PaletteItem {
  const localTarget =
    app.location ??
    (app.path && (app.path.includes(":") || app.path.startsWith("\\\\")) ? app.path : undefined);
  return {
    id: `app::${app.appId}`,
    title: app.name,
    subtitle: "Application",
    icon: { kind: "app", name: app.name, icon: icons[app.appId] ?? app.icon },
    historyTitle: app.name,
    appId: app.appId,
    dragFile: localTarget ? () => startFileDrag([localTarget]) : undefined,
    run: () => launchApp(app.appId),
    runAsAdmin: isElevatablePath(app.path) ? () => launchAppAsAdmin(app.appId) : undefined,
    openLocation: localTarget ? () => openPathLocation(localTarget) : undefined,
    shellPath: localTarget,
    toggleTaskbarPin:
      localTarget && isTaskbarPinablePath(localTarget) ? () => toggleTaskbarPin(localTarget) : undefined,
    showProperties: localTarget ? () => showPathProperties(localTarget) : undefined,
  };
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

export function quickAccessPaletteItem(entry: QuickAccessEntry): PaletteItem {
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
    quickAccessKind: entry.kind,
    dragFile: entry.path ? () => startFileDrag([entry.path]) : undefined,
    run: () => openPath(entry.path),
    openLocation: () => openPathLocation(entry.path),
    shellPath: entry.path,
    showProperties: () => showPathProperties(entry.path),
  };
}

function filePaletteItem(entry: FileEntry): PaletteItem {
  const { icon, tint } = fileAppearance(entry);
  const isPicture = !entry.isDirectory && isPicturePath(entry.path);
  return {
    id: pathItemId(entry.path, entry.isDirectory),
    title: entry.name,
    subtitle: entry.parent,
    keywords: [entry.path, entry.parent],
    icon: entry.thumbnail
      ? { kind: "image", src: entry.thumbnail, name: entry.name }
      : { kind: "tile", icon, tint },
    historyTitle: entry.name,
    isPicture,
    dragFile: entry.path ? () => startFileDrag([entry.path]) : undefined,
    run: () => openPath(entry.path),
    runAsAdmin:
      !entry.isDirectory && isElevatablePath(entry.path) ? () => runPathAsAdmin(entry.path) : undefined,
    openLocation: () => openPathLocation(entry.path),
    shellPath: entry.path,
    toggleTaskbarPin:
      !entry.isDirectory && isTaskbarPinablePath(entry.path) ? () => toggleTaskbarPin(entry.path) : undefined,
    showProperties: () => showPathProperties(entry.path),
  };
}

function fileAppearance(entry: FileEntry) {
  if (entry.isDirectory) return { icon: Folder, tint: "azure" as const };
  const extension = entry.name.split(".").pop()?.toLowerCase() ?? "";
  if (
    [
      "jpg",
      "jpeg",
      "png",
      "gif",
      "webp",
      "bmp",
      "svg",
      "ico",
      "tiff",
      "tif",
      "avif",
      "heic",
      "psd",
      "ai",
      "xd",
      "fig",
      "sketch",
      "raw",
      "cr2",
      "nef",
    ].includes(extension)
  ) {
    return { icon: FileImage, tint: "amber" as const };
  }
  if (["mp4", "mov", "mkv", "avi", "webm", "wmv", "flv", "m4v", "3gp", "ts"].includes(extension)) {
    return { icon: FileVideo, tint: "rose" as const };
  }
  if (["mp3", "wav", "flac", "m4a", "aac", "ogg", "wma", "opus", "aiff", "mid", "midi"].includes(extension)) {
    return { icon: Music, tint: "mint" as const };
  }
  if (["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "cab", "tgz"].includes(extension)) {
    return { icon: FileArchive, tint: "amber" as const };
  }
  if (
    [
      "txt",
      "md",
      "pdf",
      "doc",
      "docx",
      "xls",
      "xlsx",
      "ppt",
      "pptx",
      "csv",
      "rtf",
      "odt",
      "ods",
      "odp",
      "json",
      "xml",
      "yaml",
      "yml",
      "toml",
      "html",
      "css",
      "js",
      "ts",
      "tsx",
      "jsx",
      "rs",
      "py",
      "c",
      "cpp",
      "h",
      "cs",
      "java",
      "go",
      "sql",
      "sh",
      "bat",
      "cmd",
      "ps1",
    ].includes(extension)
  ) {
    return { icon: FileText, tint: "slate" as const };
  }
  return { icon: File, tint: "slate" as const };
}

function pathItemId(path: string, isDirectory: boolean): string {
  return `file::${isDirectory ? "d" : "f"}::${path}`;
}

/** Parses the persisted item id back into a filesystem path, when it is one. */
export function historyFilePath(id: string): string | null {
  const directoryPrefix = "file::d::";
  const filePrefix = "file::f::";
  if (id.startsWith(directoryPrefix)) return id.slice(directoryPrefix.length);
  if (id.startsWith(filePrefix)) return id.slice(filePrefix.length);
  return null;
}

function rehydrate(
  history: HistoryEntry,
  appsById: ReadonlyMap<string, AppEntry>,
  existingHistoryPaths: ReadonlySet<string>,
  icons: Readonly<Record<string, string>>,
  fileThumbnails: ReadonlyMap<string, string | null>,
): PaletteItem | null {
  if (history.id.startsWith("app::")) {
    const appId = history.id.slice(5);
    const entry = appsById.get(appId);
    return entry ? appPaletteItem(entry, icons) : null;
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
    thumbnail: fileThumbnails.get(path) ?? undefined,
  });
}
