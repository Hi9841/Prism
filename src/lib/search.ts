import type { AppEntry } from "./types";

export interface FuzzyHit<T> {
  item: T;
  score: number;
}

/**
 * Launcher-grade fuzzy scorer.
 * Returns a score (higher is better) when `query` is a subsequence of
 * `target`, or null when it isn't. Rewards prefix matches, camel-case
 * boundaries, word starts and consecutive runs; penalizes gaps.
 */
export function fuzzyScore(query: string, target: string): number | null {
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  if (q.length === 0) return null;
  if (q.length > t.length) return null;

  let qi = 0;
  let score = 0;
  let prev = -1;
  let consecutive = 0;

  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] !== q[qi]) continue;

    let s = 1;
    if (prev === ti - 1) {
      consecutive++;
      s += consecutive * 3;
    } else {
      consecutive = 0;
      s -= Math.min(8, (ti - prev - 1) * 0.5);
    }

    // Boundary bonuses: string start > camelCase > word start.
    if (ti === 0) {
      s += 12;
    } else {
      const c = target[ti];
      const pc = target[ti - 1];
      if (c !== c.toLowerCase() && c === c.toUpperCase()) {
        s += 7; // camelCase boundary
      } else if (/[\s\-_.]/.test(pc)) {
        s += 6; // word start
      }
    }

    score += s;
    prev = ti;
    qi++;
  }
  if (qi < q.length) return null;

  // Prefer tighter targets when scores are close.
  score -= t.length * 0.08;
  return score;
}

/**
 * Fuzzy search over a pool of {title, keywords} items.
 * Returns best-first, capped at `limit`.
 */
export function fuzzy<T extends { id: string; title: string; keywords?: string[] }>(
  pool: T[],
  query: string,
  opts: { limit: number },
): FuzzyHit<T>[] {
  const results: FuzzyHit<T>[] = [];
  for (const item of pool) {
    const titleScore = fuzzyScore(query, item.title);
    let score = titleScore;
    if (score === null && item.keywords) {
      let best = null;
      for (const kw of item.keywords) {
        const s = fuzzyScore(query, kw);
        if (s !== null && (best === null || s > best)) best = s;
      }
      if (best !== null) score = best - 3; // keyword-only matches rank below titles
    }
    if (score !== null) results.push({ item, score });
  }
  results.sort((a, b) => b.score - a.score);
  return results.slice(0, opts.limit);
}

export function fuzzyApps(apps: AppEntry[], query: string, limit = 8): AppEntry[] {
  const q = query.trim();
  if (q.length === 0) return [];
  const lower = q.toLowerCase();
  const tokens = lower.split(/\s+/).filter((t) => t.length > 0);
  const significantLength = normalizeText(lower).length;
  const minimumScore = significantLength >= 3 ? Math.min(24, significantLength * 3) : -Infinity;

  const results: FuzzyHit<AppEntry>[] = [];
  for (const app of dedupeApps(apps)) {
    const score = appScore(app, lower, tokens);
    if (score !== null && score >= minimumScore) results.push({ item: app, score });
  }
  results.sort((a, b) => b.score - a.score || sourcePriority(b.item.source) - sourcePriority(a.item.source));
  return results.slice(0, limit).map((h) => h.item);
}

/** Collapse aliases discovered through several Windows app registries. */
export function dedupeApps(apps: AppEntry[]): AppEntry[] {
  const preferred = new Map<string, AppEntry>();
  for (const app of apps) {
    const key = app.normalizedName || normalizeText(app.name);
    if (!key) continue;
    const current = preferred.get(key);
    if (!current || sourcePriority(app.source) > sourcePriority(current.source)) {
      preferred.set(key, app);
    }
  }
  return [...preferred.values()];
}

function sourcePriority(source?: string): number {
  switch (source) {
    case "taskbar":
      return 6;
    case "startMenu":
      return 5;
    case "desktop":
      return 4;
    case "appsFolder":
      return 3;
    case "registry":
      return 2;
    case "programs":
      return 1;
    default:
      return 0;
  }
}

function appScore(app: AppEntry, lowerQuery: string, tokens: string[]): number | null {
  const lowerName = app.name.toLowerCase();

  // Punctuation-insensitive exact match: "visualstudiocode" matches
  // "Visual Studio Code" no matter how the user typed it.
  if (app.normalizedName && normalizeText(lowerQuery) === app.normalizedName) {
    return 100;
  }

  const nameScore = matchTarget(lowerName, tokens);
  if (nameScore !== null) {
    // Exact and prefix application-name matches rank first.
    if (lowerName === lowerQuery) return nameScore + 40;
    if (lowerName.startsWith(lowerQuery)) return nameScore + 20;
    return nameScore;
  }

  // Fall back to metadata: exe names, folder names, publisher, aliases.
  let best: number | null = null;
  for (const keyword of app.keywords ?? []) {
    const score = matchTarget(keyword.toLowerCase(), tokens);
    if (score !== null && (best === null || score > best)) best = score;
  }
  if (best !== null) return best - 4; // keyword-only matches rank below names
  return null;
}

/**
 * Whole-query subsequence match for single tokens; per-token summed
 * subsequence match for multi-word queries (every word must hit, which
 * gives partial-word matching like "shipping" in "FortniteClient-Win64-
 * Shipping" and "studio code" in "Visual Studio Code").
 */
function matchTarget(lowerTarget: string, tokens: string[]): number | null {
  if (tokens.length === 1) return fuzzyScore(tokens[0], lowerTarget);
  let total = 0;
  for (const token of tokens) {
    const score = fuzzyScore(token, lowerTarget);
    if (score === null) return null;
    total += score;
  }
  return total;
}

function normalizeText(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, "");
}
