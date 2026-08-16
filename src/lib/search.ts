import type { AppEntry } from "./types";

export interface FuzzyHit<T> {
  item: T;
  score: number;
}

/** Whitespace plus the common name separators (space, tab, -, _, .). */
function isWordSeparator(code: number): boolean {
  return code === 32 || code === 9 || code === 10 || code === 13 || code === 45 || code === 95 || code === 46;
}

/**
 * Launcher-grade fuzzy scorer.
 * Returns a score (higher is better) when `query` is a subsequence of
 * `target`, or null when it isn't. Rewards prefix matches, camel-case
 * boundaries, word starts and consecutive runs; penalizes gaps.
 */
export function fuzzyScore(query: string, target: string): number | null {
  return fuzzyScoreCore(query.toLowerCase(), target.toLowerCase(), target);
}

/**
 * Core scorer over pre-lowercased inputs. `boundaryTarget` carries the
 * original-case target for camelCase detection, or null when the target is
 * already known lowercase and the check would be dead work.
 */
function fuzzyScoreCore(q: string, t: string, boundaryTarget: string | null): number | null {
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

    // Boundary bonuses: string start > camelCase > word start. Separators are
    // case-agnostic, so the lowercase copy serves the word-start check.
    if (ti === 0) {
      s += 12;
    } else if (boundaryTarget !== null) {
      const c = boundaryTarget[ti];
      if (c !== c.toLowerCase() && c === c.toUpperCase()) {
        s += 7; // camelCase boundary
      } else if (isWordSeparator(t.charCodeAt(ti - 1))) {
        s += 6; // word start
      }
    } else if (isWordSeparator(t.charCodeAt(ti - 1))) {
      s += 6; // word start
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

/**
 * Per-app search metadata, computed once per AppEntry object (WeakMap-keyed
 * on entry identity). Lowercasing every name and keyword on every keystroke
 * was the dominant allocation churn on the search hot path; entries replaced
 * by a refresh drop out of the map automatically.
 */
interface PreparedApp {
  lowerName: string;
  lowerKeywords: readonly string[];
  sourceRank: number;
}

const preparedApps = new WeakMap<AppEntry, PreparedApp>();

function prepareApp(app: AppEntry): PreparedApp {
  let prepared = preparedApps.get(app);
  if (!prepared) {
    prepared = {
      lowerName: app.name.toLowerCase(),
      lowerKeywords: (app.keywords ?? []).map((keyword) => keyword.toLowerCase()),
      sourceRank: sourcePriority(app.source),
    };
    preparedApps.set(app, prepared);
  }
  return prepared;
}

interface RankedHit {
  item: AppEntry;
  score: number;
  rank: number;
}

/**
 * Keeps `top` sorted best-first (score desc, then source rank desc) and
 * capped at `limit`. Bounded insertion avoids sorting the full match set,
 * which for one- and two-character queries is most of the app pool.
 */
function insertTopK(top: RankedHit[], hit: RankedHit, limit: number): void {
  let lo = 0;
  let hi = top.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const existing = top[mid];
    const existingFirst =
      existing.score > hit.score || (existing.score === hit.score && existing.rank >= hit.rank);
    if (existingFirst) lo = mid + 1;
    else hi = mid;
  }
  if (lo >= limit) return;
  top.splice(lo, 0, hit);
  if (top.length > limit) top.pop();
}

export function fuzzyApps(
  apps: AppEntry[],
  query: string,
  limit = 8,
  opts: { preDeduped?: boolean } = {},
): AppEntry[] {
  const q = query.trim();
  if (q.length === 0) return [];
  const lower = q.toLowerCase();
  const tokens = lower.split(/\s+/).filter((t) => t.length > 0);
  const normalizedQuery = normalizeText(lower);
  const significantLength = normalizedQuery.length;
  const minimumScore = significantLength >= 3 ? Math.min(24, significantLength * 3) : -Infinity;

  // Callers that already deduplicated (memoized) the list skip the map build
  // on the per-keystroke hot path.
  const pool = opts.preDeduped ? apps : dedupeApps(apps);
  const top: RankedHit[] = [];
  for (const app of pool) {
    const prepared = prepareApp(app);
    const score = appScore(app, prepared, lower, tokens, normalizedQuery);
    if (score !== null && score >= minimumScore) {
      insertTopK(top, { item: app, score, rank: prepared.sourceRank }, limit);
    }
  }
  return top.map((hit) => hit.item);
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
    case "appPaths":
    case "applications":
      return 2;
    case "programs":
      return 1;
    default:
      return 0;
  }
}

function appScore(
  app: AppEntry,
  prepared: PreparedApp,
  lowerQuery: string,
  tokens: string[],
  normalizedQuery: string,
): number | null {
  const lowerName = prepared.lowerName;

  // Punctuation-insensitive exact match: "visualstudiocode" matches
  // "Visual Studio Code" no matter how the user typed it.
  if (app.normalizedName && normalizedQuery === app.normalizedName) {
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
  for (const keyword of prepared.lowerKeywords) {
    const score = matchTarget(keyword, tokens);
    if (score !== null && (best === null || score > best)) best = score;
  }
  if (best !== null) return best - 4; // keyword-only matches rank below names
  return null;
}

/**
 * Whole-query subsequence match for single tokens; per-token summed
 * subsequence match for multi-word queries (every word must hit, which
 * gives partial-word matching like "shipping" in "FortniteClient-Win64-
 * Shipping" and "studio code" in "Visual Studio Code"). Tokens and targets
 * arrive pre-lowercased by the callers.
 */
function matchTarget(lowerTarget: string, tokens: string[]): number | null {
  if (tokens.length === 1) return fuzzyScoreCore(tokens[0], lowerTarget, null);
  let total = 0;
  for (const token of tokens) {
    const score = fuzzyScoreCore(token, lowerTarget, null);
    if (score === null) return null;
    total += score;
  }
  return total;
}

function normalizeText(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, "");
}
