export const MIN_BACKGROUND_CHECK_INTERVAL_MS = 5 * 60 * 1000;
/**
 * Even explicit opens must not turn Win-key spam into repeated no-cache
 * requests. Forced checks keep their freshness guarantee without firing a
 * network request on every single palette presentation.
 */
export const MIN_FORCED_CHECK_INTERVAL_MS = 60 * 1000;

export function shouldCheckForUpdate(lastCheckAt: number, now: number, force: boolean): boolean {
  const interval = force ? MIN_FORCED_CHECK_INTERVAL_MS : MIN_BACKGROUND_CHECK_INTERVAL_MS;
  return now - lastCheckAt >= interval;
}

/**
 * True when the released version is below the installed one. Releases can be
 * deleted and the feed repointed to an older build; users stuck on a removed
 * version need the offer to roll back, not a silent "up to date".
 *
 * Versions are plain MAJOR.MINOR.PATCH-like strings (an optional leading `v`
 * is tolerated), so numeric segment comparison is enough. Prerelease suffixes
 * are treated as their numeric prefix, which is conservative.
 */
export function isDowngrade(currentVersion: string, updateVersion: string): boolean {
  const parse = (value: string): number[] =>
    value
      .replace(/^v/i, "")
      .split(/[.+-]/)
      .map((part) => Number.parseInt(part, 10) || 0);
  const current = parse(currentVersion);
  const update = parse(updateVersion);
  for (let i = 0; i < Math.max(current.length, update.length); i++) {
    const diff = (current[i] ?? 0) - (update[i] ?? 0);
    if (diff !== 0) return diff > 0;
  }
  return false;
}
