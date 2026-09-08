/**
 * The start-menu view's model: what the two-column menu shows for an empty
 * query. Pure decision logic - no React, no state - so every grouping and
 * ranking rule is unit-testable through this single seam, mirroring the
 * palette's `sections.ts`.
 *
 * Layout follows the classic start-menu pattern: a pinned column with the
 * user's frequent apps and quick-access folders, and an "All apps" column
 * that mirrors the Start Menu folder tree on disk.
 */

import { sortApps } from "../../lib/emoji";
import type { AppEntry, HistoryEntry, PaletteItem } from "../../lib/types";
import { appPaletteItem } from "./sections";

/** Cap on the frequent-apps rows; matching a readable column height. */
export const FREQUENT_LIMIT = 6;

export interface ProgramGroup {
  /** Stable id: `menu-group-` plus a slug of the folder label. */
  id: string;
  /** Start Menu folder this group mirrors, e.g. `Accessories › Games`. */
  label: string;
  items: PaletteItem[];
}

export interface MenuModel {
  pinned: PaletteItem[];
  frequent: PaletteItem[];
  programGroups: ProgramGroup[];
  /** Groups flattened in render order - the keyboard navigation order. */
  programFlat: PaletteItem[];
}

/** Everything the menu model may read, in one place. */
export interface MenuSources {
  /** Deduplicated app entries (the provider dedupes before calling). */
  apps: AppEntry[];
  /** Resolved pinned entries, in user order. */
  pinnedApps: AppEntry[];
  /** `settings.pinnedApps` - ids excluded from the frequent ranking. */
  pinnedAppIds: readonly string[];
  history: HistoryEntry[];
  appIcons: Readonly<Record<string, string>>;
}

export function buildMenuModel(sources: MenuSources): MenuModel {
  const { apps, pinnedApps, pinnedAppIds, history, appIcons } = sources;

  const pinned = pinnedApps.map((entry) => appPaletteItem(entry, appIcons));

  // Frequent = launch counts from history, most-launched first, recency as
  // the tiebreaker. Pinned apps never repeat in the frequent rows.
  const counts = new Map<string, { count: number; lastTs: number }>();
  for (const entry of history) {
    if (!entry.id.startsWith("app::")) continue;
    const appId = entry.id.slice(5);
    if (pinnedAppIds.includes(appId)) continue;
    const current = counts.get(appId);
    if (current) {
      current.count += 1;
      current.lastTs = Math.max(current.lastTs, entry.ts);
    } else {
      counts.set(appId, { count: 1, lastTs: entry.ts });
    }
  }
  const appsById = new Map(apps.map((entry) => [entry.appId, entry]));
  const ranked = [...counts.entries()]
    .sort((a, b) => b[1].count - a[1].count || b[1].lastTs - a[1].lastTs)
    .slice(0, FREQUENT_LIMIT);
  const frequent: PaletteItem[] = [];
  for (const [appId] of ranked) {
    const entry = appsById.get(appId);
    if (entry) frequent.push(appPaletteItem(entry, appIcons));
  }

  const programGroups = buildProgramGroups(apps, appIcons);
  const programFlat: PaletteItem[] = programGroups.flatMap((group) => group.items);

  return { pinned, frequent, programGroups, programFlat };
}

/** Marker folder that separates the Start Menu tree from its root label. */
const PROGRAMS_MARKER = "\\programs\\";

/**
 * Mirrors the Start Menu folder structure: a shortcut under
 * `...\Start Menu\Programs\Accessories\Tools\Note.lnk` lands in the
 * `Accessories › Tools` group; shortcuts directly under `Programs` land in
 * `Programs`; anything without a Start Menu location lands in `Other apps`,
 * which always sorts last.
 */
export function programGroupLabel(app: Pick<AppEntry, "location" | "source">): string {
  if (!app.location) return "Other apps";
  const normalized = app.location.replace(/\//g, "\\");
  const markerIndex = normalized.toLowerCase().indexOf(PROGRAMS_MARKER);
  if (markerIndex < 0) return "Other apps";
  const relative = normalized.slice(markerIndex + PROGRAMS_MARKER.length);
  const boundary = relative.lastIndexOf("\\");
  if (boundary <= 0) return "Programs";
  return relative.slice(0, boundary).split("\\").join(" › ");
}

function buildProgramGroups(apps: AppEntry[], appIcons: Readonly<Record<string, string>>): ProgramGroup[] {
  const byLabel = new Map<string, AppEntry[]>();
  for (const app of apps) {
    const label = programGroupLabel(app);
    const bucket = byLabel.get(label);
    if (bucket) bucket.push(app);
    else byLabel.set(label, [app]);
  }

  const groups = [...byLabel.entries()].map(([label, entries]) => ({
    id: `menu-group-${slug(label)}`,
    label,
    // `Other apps` groups keep catalog order inside; folder groups sort A→Z.
    items: (label === "Other apps" ? entries : sortApps(entries)).map((entry) =>
      appPaletteItem(entry, appIcons),
    ),
  }));

  return groups.sort((a, b) => {
    if (a.label === "Other apps") return 1;
    if (b.label === "Other apps") return -1;
    return a.label.localeCompare(b.label, undefined, { sensitivity: "base" });
  });
}

function slug(label: string): string {
  return (
    label
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/(^-|-$)/g, "") || "other"
  );
}
