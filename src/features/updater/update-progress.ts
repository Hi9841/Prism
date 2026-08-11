export function updatePercent(downloadedBytes: number, totalBytes?: number): number | null {
  if (!totalBytes || totalBytes <= 0) return null;
  const percent = Math.round((Math.max(0, downloadedBytes) / totalBytes) * 100);
  return Math.min(100, percent);
}
