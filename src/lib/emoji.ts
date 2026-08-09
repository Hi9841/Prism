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

export function sortApps(apps: AppEntry[]): AppEntry[] {
  return [...apps].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
}
