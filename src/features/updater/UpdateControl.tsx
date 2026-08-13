import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { AlertCircle, ArrowDownToLine, LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { inTauri, onToggleRequest } from "../../lib/bridge";
import { useApp } from "../../state/app";
import { shouldCheckForUpdate } from "./update-policy";
import { updatePercent } from "./update-progress";

const CHECK_INTERVAL_MS = 60 * 60 * 1000;
const NETWORK_TIMEOUT_MS = 15 * 1000;
const DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1000;

type UpdateViewState =
  | { phase: "hidden" }
  | { phase: "available"; version: string }
  | { phase: "downloading"; version: string; downloadedBytes: number; totalBytes?: number }
  | { phase: "installing"; version: string }
  | { phase: "restarting"; version: string }
  | { phase: "failed"; version: string };

export function UpdateControl() {
  const { showToast } = useApp();
  const [viewState, setViewState] = useState<UpdateViewState>({ phase: "hidden" });
  const updateRef = useRef<Update | null>(null);
  const checkInFlightRef = useRef<Promise<void> | null>(null);
  const installInFlightRef = useRef(false);
  const disposedRef = useRef(false);
  const lastCheckAtRef = useRef(0);
  const forceCheckPendingRef = useRef(false);

  const checkForUpdate = useCallback((force = false) => {
    if (!inTauri || installInFlightRef.current) return;
    if (checkInFlightRef.current) {
      if (force) forceCheckPendingRef.current = true;
      return;
    }
    const now = Date.now();
    // Opens are explicit requests for current release information, but the
    // policy floor keeps Win-key spam from firing repeated no-cache
    // requests. The request runs asynchronously and never blocks native
    // presentation.
    if (!shouldCheckForUpdate(lastCheckAtRef.current, now, force)) return;
    lastCheckAtRef.current = now;

    const pending = check({
      timeout: NETWORK_TIMEOUT_MS,
      headers: force ? { "Cache-Control": "no-cache", Pragma: "no-cache" } : undefined,
    })
      .then(async (availableUpdate) => {
        if (disposedRef.current) {
          await availableUpdate?.close();
          return;
        }
        if (!availableUpdate) {
          const previousUpdate = updateRef.current;
          updateRef.current = null;
          if (previousUpdate) await previousUpdate.close().catch(() => {});
          setViewState({ phase: "hidden" });
          return;
        }
        const previousUpdate = updateRef.current;
        updateRef.current = availableUpdate;
        if (previousUpdate) await previousUpdate.close().catch(() => {});
        setViewState({ phase: "available", version: availableUpdate.version });
      })
      .catch(() => {
        // Background checks are intentionally quiet. The next palette open or
        // hourly retry handles temporary network and GitHub failures.
      })
      .finally(() => {
        checkInFlightRef.current = null;
        if (forceCheckPendingRef.current && !disposedRef.current && !installInFlightRef.current) {
          forceCheckPendingRef.current = false;
          checkForUpdate(true);
        }
      });
    checkInFlightRef.current = pending;
  }, []);

  useEffect(() => {
    disposedRef.current = false;
    checkForUpdate();
    const interval = window.setInterval(checkForUpdate, CHECK_INTERVAL_MS);
    const offToggle = onToggleRequest((request) => {
      if (request.open) checkForUpdate(true);
    });
    return () => {
      disposedRef.current = true;
      forceCheckPendingRef.current = false;
      window.clearInterval(interval);
      offToggle();
      const update = updateRef.current;
      updateRef.current = null;
      update?.close().catch(() => {});
    };
  }, [checkForUpdate]);

  const installUpdate = useCallback(async () => {
    const update = updateRef.current;
    if (!update || viewState.phase === "downloading" || viewState.phase === "installing") return;

    let downloadedBytes = 0;
    installInFlightRef.current = true;
    setViewState({
      phase: "downloading",
      version: update.version,
      downloadedBytes,
    });
    try {
      await update.downloadAndInstall(
        (event) => {
          if (disposedRef.current) return;
          if (event.event === "Started") {
            setViewState({
              phase: "downloading",
              version: update.version,
              downloadedBytes,
              totalBytes: event.data.contentLength,
            });
          } else if (event.event === "Progress") {
            downloadedBytes += event.data.chunkLength;
            setViewState((current) => ({
              phase: "downloading",
              version: update.version,
              downloadedBytes,
              totalBytes: current.phase === "downloading" ? current.totalBytes : undefined,
            }));
          } else {
            setViewState({ phase: "installing", version: update.version });
          }
        },
        { timeout: DOWNLOAD_TIMEOUT_MS },
      );
      if (disposedRef.current) return;
      setViewState({ phase: "restarting", version: update.version });
      await relaunch();
    } catch (error) {
      console.error("Prism update failed", error);
      if (disposedRef.current) return;
      setViewState({ phase: "failed", version: update.version });
      showToast("Update failed", "Check your connection and try again");
    } finally {
      installInFlightRef.current = false;
    }
  }, [showToast, viewState.phase]);

  if (viewState.phase === "hidden") return null;

  const version = viewState.version.replace(/^v/i, "");
  const busy =
    viewState.phase === "downloading" || viewState.phase === "installing" || viewState.phase === "restarting";
  const percent =
    viewState.phase === "downloading" ? updatePercent(viewState.downloadedBytes, viewState.totalBytes) : null;
  const label =
    viewState.phase === "available"
      ? `Update v${version}`
      : viewState.phase === "downloading"
        ? percent === null
          ? "Downloading"
          : `Update ${percent}%`
        : viewState.phase === "installing"
          ? "Installing"
          : viewState.phase === "restarting"
            ? "Restarting"
            : "Retry update";
  const title =
    viewState.phase === "failed"
      ? `Retry Prism v${version} update`
      : busy
        ? `${label} Prism v${version}`
        : `Install Prism v${version}`;

  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      aria-busy={busy}
      disabled={busy}
      onClick={installUpdate}
      className={`focus-ring press inline-flex h-8 w-28 min-w-0 items-center justify-center gap-1.5 rounded-[7px] px-2.5 text-[11px] font-semibold ${
        viewState.phase === "failed"
          ? "bg-danger-soft text-danger hover:opacity-90"
          : busy
            ? "cursor-wait bg-surface text-fg-secondary"
            : "cursor-pointer bg-accent-soft text-accent hover:bg-surface-active hover:text-fg"
      }`}
    >
      <span className="relative grid h-3.5 w-3.5 shrink-0 place-items-center" aria-hidden="true">
        <LoaderCircle
          className={`icon-swap absolute inset-0 h-3.5 w-3.5 ${
            busy ? "scale-100 opacity-100 blur-[0px]" : "scale-[0.25] opacity-0 blur-[4px]"
          } ${busy ? "animate-spin" : ""}`}
        />
        <AlertCircle
          className={`icon-swap absolute inset-0 h-3.5 w-3.5 ${
            viewState.phase === "failed"
              ? "scale-100 opacity-100 blur-[0px]"
              : "scale-[0.25] opacity-0 blur-[4px]"
          }`}
        />
        <ArrowDownToLine
          className={`icon-swap h-3.5 w-3.5 ${
            !busy && viewState.phase !== "failed"
              ? "scale-100 opacity-100 blur-[0px]"
              : "scale-[0.25] opacity-0 blur-[4px]"
          }`}
        />
      </span>
      <span className="truncate tabular-nums" aria-live="polite">
        {label}
      </span>
    </button>
  );
}
