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
