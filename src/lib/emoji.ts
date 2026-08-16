import type { AppEntry } from "./types";

/** Deterministic monogram hue (0-1) derived from a name. */
export function hueForName(name: string): number {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return (h % 360) / 360;
}

export function nameToMonogram(name: string): string {
  const words = name
    .replace(/[()[\]{}]/g, " ")
    .split(/\s+/)
    .filter((w) => /[a-z0-9]/i.test(w));
  if (words.length === 0) return "?";
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[1][0]).toUpperCase();
}

// The localeCompare sort of the whole app list only has to run when the list
// itself changes; the idle view is rebuilt on every icon batch, thumbnail, and
// history validation, so cache the sorted copy against the input identity.
const sortedAppsCache = new WeakMap<AppEntry[], AppEntry[]>();

export function sortApps(apps: AppEntry[]): AppEntry[] {
  const cached = sortedAppsCache.get(apps);
  if (cached) return cached;
  const sorted = [...apps].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
  sortedAppsCache.set(apps, sorted);
  return sorted;
}
