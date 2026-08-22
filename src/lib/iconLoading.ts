const APP_ICON_RETRY_DELAYS_MS = [200, 800] as const;

interface AppIconRequestState {
  appsLoaded: boolean;
  icons: Readonly<Record<string, string>>;
  settled: ReadonlySet<string>;
  inFlight: ReadonlySet<string>;
  attempts: ReadonlyMap<string, number>;
}

export function selectAppIconRequestIds(ids: readonly string[], state: AppIconRequestState): string[] {
  if (!state.appsLoaded) return [];
  const maximumAttempts = APP_ICON_RETRY_DELAYS_MS.length + 1;
  return ids.filter(
    (id) =>
      !(id in state.icons) &&
      !state.settled.has(id) &&
      !state.inFlight.has(id) &&
      (state.attempts.get(id) ?? 0) < maximumAttempts,
  );
}

export function appIconRetryDelay(attempt: number): number | null {
  return APP_ICON_RETRY_DELAYS_MS[attempt - 1] ?? null;
}
