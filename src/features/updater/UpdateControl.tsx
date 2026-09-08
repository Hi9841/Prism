import { check, type Update } from "@tauri-apps/plugin-updater";
import { AlertCircle, ArrowDownToLine, LoaderCircle, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { inTauri, onToggleRequest } from "../../lib/bridge";
import { useApp } from "../../state/app";
import { isDowngrade, shouldCheckForUpdate } from "./update-policy";
import { updatePercent } from "./update-progress";

const CHECK_INTERVAL_MS = 60 * 60 * 1000;
const NETWORK_TIMEOUT_MS = 15 * 1000;
const DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1000;

type UpdateViewState =
  | { phase: "hidden" }
  | { phase: "available"; version: string; downgrade: boolean }
  | { phase: "saving"; version: string; downgrade: boolean }
  | {
      phase: "downloading";
      version: string;
      downgrade: boolean;
      downloadedBytes: number;
      totalBytes?: number;
    }
  | { phase: "installing"; version: string; downgrade: boolean }
  | { phase: "failed"; version: string; downgrade: boolean };

export function UpdateControl() {
  const { showToast, flushPersistence } = useApp();
  const [viewState, setViewState] = useState<UpdateViewState>({ phase: "hidden" });
  const updateRef = useRef<Update | null>(null);
  const checkInFlightRef = useRef<Promise<void> | null>(null);
  const installInFlightRef = useRef(false);
  const installGenerationRef = useRef(0);
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
    const generation = installGenerationRef.current;

    const pending = check({
      timeout: NETWORK_TIMEOUT_MS,
      target: "windows-x86_64-nsis",
      // Releases can be deleted and the feed repointed lower. Allow the check
      // to report a version below the installed one so users on a removed
      // build are offered the roll back instead of staying stuck forever.
      allowDowngrades: true,
      headers: force ? { "Cache-Control": "no-cache", Pragma: "no-cache" } : undefined,
    })
      .then(async (availableUpdate) => {
        if (disposedRef.current || generation !== installGenerationRef.current) {
          if (availableUpdate !== updateRef.current) await availableUpdate?.close().catch(() => {});
          return;
        }
        if (!availableUpdate) {
          const previousUpdate = updateRef.current;
          updateRef.current = null;
          setViewState({ phase: "hidden" });
          if (previousUpdate) void previousUpdate.close().catch(() => {});
          return;
        }
        const previousUpdate = updateRef.current;
        updateRef.current = availableUpdate;
        setViewState({
          phase: "available",
          version: availableUpdate.version,
          downgrade: isDowngrade(availableUpdate.currentVersion, availableUpdate.version),
        });
        if (previousUpdate && previousUpdate !== availableUpdate) void previousUpdate.close().catch(() => {});
      })
      .catch((error) => {
        if (!disposedRef.current && generation === installGenerationRef.current) {
          console.error("Prism update check failed", error);
          setViewState({ phase: "failed", version: "latest", downgrade: false });
        }
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
      if (!installInFlightRef.current) {
        updateRef.current = null;
        update?.close().catch(() => {});
      }
    };
  }, [checkForUpdate]);

  const installUpdate = useCallback(async () => {
    if (installInFlightRef.current) return;
    const update = updateRef.current;
    if (viewState.phase === "failed" && !update) {
      checkForUpdate(true);
      return;
    }
    if (!update || viewState.phase === "downloading" || viewState.phase === "installing") return;
    const downgrade = isDowngrade(update.currentVersion, update.version);

    let downloadedBytes = 0;
    installInFlightRef.current = true;
    installGenerationRef.current += 1;
    let saving = true;
    setViewState({ phase: "saving", version: update.version, downgrade });
    try {
      await flushPersistence();
      if (disposedRef.current) return;
      saving = false;
      setViewState({
        phase: "downloading",
        version: update.version,
        downgrade,
        downloadedBytes,
      });
      await update.download(
        (event) => {
          if (disposedRef.current) return;
          if (event.event === "Started") {
            setViewState({
              phase: "downloading",
              version: update.version,
              downgrade,
              downloadedBytes,
              totalBytes: event.data.contentLength,
            });
          } else if (event.event === "Progress") {
            downloadedBytes += event.data.chunkLength;
            setViewState((current) => ({
              phase: "downloading",
              version: update.version,
              downgrade,
              downloadedBytes,
              totalBytes: current.phase === "downloading" ? current.totalBytes : undefined,
            }));
          }
        },
        { timeout: DOWNLOAD_TIMEOUT_MS },
      );
      if (disposedRef.current) return;
      // Settings can change during a long download. Flush again immediately
      // before handing control to the native installer, which exits Prism.
      saving = true;
      setViewState({ phase: "saving", version: update.version, downgrade });
      await flushPersistence();
      if (disposedRef.current) return;
      saving = false;
      setViewState({ phase: "installing", version: update.version, downgrade });
      await update.install();
      // On Windows, Tauri's native updater launches the NSIS installer with
      // restart enabled and exits Prism from Rust. This line is only reached
      // by platforms whose updater returns normally; the process must not
      // call relaunch again because that races the installer and can start
      // the old executable while it is being replaced.
    } catch (error) {
      if (disposedRef.current) return;
      setViewState({ phase: "failed", version: update.version, downgrade });
      showToast(saving ? "Settings not saved" : "Update failed", String(error), "error");
    } finally {
      installInFlightRef.current = false;
      if (disposedRef.current) {
        if (updateRef.current === update) updateRef.current = null;
        await update.close().catch(() => {});
      }
    }
  }, [checkForUpdate, showToast, flushPersistence, viewState.phase]);

  if (viewState.phase === "hidden") {
    return (
      <button
        type="button"
        title="Check for updates"
        aria-label="Check for updates"
        onClick={() => checkForUpdate(true)}
        className="focus-ring press relative grid h-8 w-8 place-items-center rounded-[10px] text-fg-quiet after:absolute after:-inset-1.5 after:content-[''] hover:bg-surface-hover hover:text-fg"
      >
        <RefreshCw className="h-3.5 w-3.5" />
      </button>
    );
  }

  const version = viewState.version.replace(/^v/i, "");
  const downgrade = viewState.downgrade;
  const busy =
    viewState.phase === "saving" || viewState.phase === "downloading" || viewState.phase === "installing";
  const percent =
    viewState.phase === "downloading" ? updatePercent(viewState.downloadedBytes, viewState.totalBytes) : null;
  const label =
    viewState.phase === "available"
      ? `${downgrade ? "Downgrade" : "Update"} v${version}`
      : viewState.phase === "saving"
        ? "Saving settings"
        : viewState.phase === "downloading"
          ? percent === null
            ? "Downloading"
            : `Update ${percent}%`
          : viewState.phase === "installing"
            ? "Installing"
            : downgrade
              ? "Retry downgrade"
              : "Retry update";
  const title =
    viewState.phase === "failed"
      ? `Retry Prism v${version} ${downgrade ? "downgrade" : "update"}`
      : busy
        ? `${label} Prism v${version}`
        : downgrade
          ? `Roll back to Prism v${version}`
          : `Install Prism v${version}`;

  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      aria-busy={busy}
      disabled={busy}
      onClick={installUpdate}
      className={`focus-ring press relative inline-flex h-8 shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-[10px] px-2.5 text-[11px] font-semibold after:absolute after:-inset-y-1.5 after:-inset-x-1 after:content-[''] ${
        viewState.phase === "failed"
          ? "bg-danger-soft text-danger hover:opacity-90"
          : busy
            ? "cursor-wait bg-surface text-fg-secondary"
            : "cursor-pointer bg-accent-soft text-fg hover:bg-surface-active"
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
      <span className="tabular-nums" aria-live="polite">
        {label}
      </span>
    </button>
  );
}
