export const MIN_BACKGROUND_CHECK_INTERVAL_MS = 5 * 60 * 1000;

export function shouldCheckForUpdate(lastCheckAt: number, now: number, force: boolean): boolean {
  return force || now - lastCheckAt >= MIN_BACKGROUND_CHECK_INTERVAL_MS;
}
