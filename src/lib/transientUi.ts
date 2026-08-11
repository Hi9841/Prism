const DISMISS_EVENT = "prism:dismiss-transient-ui";

export function dismissTransientUi(): void {
  document.dispatchEvent(new CustomEvent(DISMISS_EVENT));
}

export function onTransientUiDismiss(callback: () => void): () => void {
  document.addEventListener(DISMISS_EVENT, callback);
  return () => document.removeEventListener(DISMISS_EVENT, callback);
}
