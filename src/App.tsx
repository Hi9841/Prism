import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import packageInfo from "../package.json";
import { SettingsSheet } from "./components/SettingsSheet";
import { ToastStack } from "./components/Toast";
import { Palette } from "./features/palette/Palette";
import {
  getAppVersion,
  hidePaletteWindow,
  inTauri,
  isWindowVisible,
  onToggleRequest,
  onWindowFocused,
  presentPaletteWindow,
} from "./lib/bridge";
import {
  hidePaletteInvocation,
  LAUNCHER_MOTION_MS,
  type LauncherPhase,
  reduceLauncherPhase,
} from "./lib/launcherMotion";
import { dismissTransientUi } from "./lib/transientUi";
import { AppProvider, useApp } from "./state/app";
import { PaletteProvider, usePalette } from "./state/palette";

const BUNDLE_VERSION_KEY = "prism:last-native-version";

function refreshStaleBundle(nativeVersion: string) {
  if (!nativeVersion || nativeVersion === packageInfo.version) {
    try {
      window.sessionStorage.removeItem(BUNDLE_VERSION_KEY);
    } catch {
      // Storage can be unavailable in a restricted WebView context.
    }
    return;
  }

  try {
    if (window.sessionStorage.getItem(BUNDLE_VERSION_KEY) === nativeVersion) return;
    window.sessionStorage.setItem(BUNDLE_VERSION_KEY, nativeVersion);
  } catch {
    return;
  }

  // A new native binary can be installed while WebView2 still has the old
  // entry document cached. A version query makes the next load address the
  // new bundled document without rebuilding the reusable window.
  const url = new URL(window.location.href);
  url.searchParams.set("prism-version", nativeVersion);
  window.location.replace(url);
}

function Launcher() {
  const app = useApp();
  const palette = usePalette();
  const { reset } = palette;
  const { setOpenSettings } = app;
  const [phase, setPhase] = useState<LauncherPhase>(inTauri ? "hidden" : "visible");
  const phaseRef = useRef(phase);
  const visible = phase !== "hidden";
  const visibleRef = useRef(false);
  const blurCheck = useRef<number | null>(null);
  const closeTimer = useRef<number | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    getAppVersion()
      .then(refreshStaleBundle)
      .catch(() => {});
  }, []);

  phaseRef.current = phase;
  visibleRef.current = visible;

  const clearCloseTimer = useCallback(() => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  }, []);

  const hide = useCallback(
    (origin: "native" | "web") => {
      dismissTransientUi();
      const event = origin === "native" ? "native-close" : "web-close";
      const next = reduceLauncherPhase(phaseRef.current, event);
      if (next === phaseRef.current) return;
      clearCloseTimer();
      setPhase(next);
      const invocation = hidePaletteInvocation(origin, next);
      if (invocation === "instant") hidePaletteWindow(false).catch(() => {});
      if (invocation === "deferred") hidePaletteWindow(true).catch(() => {});
      if (next === "closing") {
        closeTimer.current = window.setTimeout(() => {
          closeTimer.current = null;
          setPhase((current) => reduceLauncherPhase(current, "exit-finished"));
        }, LAUNCHER_MOTION_MS);
      }
    },
    [clearCloseTimer],
  );

  const hideFromWeb = useCallback(() => hide("web"), [hide]);

  const focusSearch = useCallback(() => {
    document.querySelector<HTMLInputElement>("[data-prism-search]")?.focus();
  }, []);

  // Rust-side: global hotkey pressed, or user clicked away (blur).
  useEffect(() => {
    const offToggle = onToggleRequest((request) => {
      if (!request.open) {
        hide("native");
        return;
      }
      dismissTransientUi();
      // Fresh state on every open: no leftover query, selection or
      // settings overlay - the palette starts from zero.
      reset();
      setOpenSettings(false);
      clearCloseTimer();
      const next = reduceLauncherPhase(phaseRef.current, "open");
      setPhase(next);
      if (next === "visible") {
        presentPaletteWindow()
          .then((presented) => {
            if (!presented) {
              setPhase((current) => reduceLauncherPhase(current, "present-failed"));
              return;
            }
            focusSearch();
          })
          .catch(() => {
            setPhase((current) => reduceLauncherPhase(current, "present-failed"));
          });
      }
    });
    const offBlur = onWindowFocused((focused) => {
      if (focused) {
        if (blurCheck.current !== null) window.clearTimeout(blurCheck.current);
        blurCheck.current = null;
        return;
      }
      if (blurCheck.current !== null) window.clearTimeout(blurCheck.current);
      blurCheck.current = window.setTimeout(() => {
        blurCheck.current = null;
        isWindowVisible()
          .then((nativeVisible) => {
            if (!nativeVisible) {
              // Instant native hide (item launch) already took the window.
              dismissTransientUi();
              clearCloseTimer();
              setPhase("hidden");
              return;
            }
            // Click-away: the window is still up for the shared hide delay.
            // prism-toggle usually starts the exit; this covers a missed event.
            if (phaseRef.current === "visible" || phaseRef.current === "preparing") {
              hide("native");
            }
          })
          .catch(() => {});
      }, 60);
    });
    document.addEventListener("prism:close", hideFromWeb);
    // Cold-start sync: if the webview loads after the window was already
    // shown (first toggle racing the page load), reflect the real state so
    // the palette isn't stuck invisible inside a visible window.
    isWindowVisible()
      .then((isVisible) => {
        // A webview refresh can leave the native window visible while Rust's
        // presentation path has not run. Re-enter the preparing phase so the
        // native reconciliation moves Prism and the taskbar together.
        if (isVisible && !visibleRef.current) setPhase(inTauri ? "preparing" : "visible");
      })
      .catch(() => {});
    return () => {
      offToggle();
      offBlur();
      if (blurCheck.current !== null) window.clearTimeout(blurCheck.current);
      document.removeEventListener("prism:close", hideFromWeb);
    };
  }, [clearCloseTimer, focusSearch, hide, hideFromWeb, reset, setOpenSettings]);

  useEffect(() => () => clearCloseTimer(), [clearCloseTimer]);

  // Commit the fully prepared DOM while the native window is still hidden.
  // Only then present the window and begin the entrance on the next frame.
  useLayoutEffect(() => {
    if (phase !== "preparing") return;
    let cancelled = false;
    presentPaletteWindow()
      .then((presented) => {
        if (cancelled) return;
        if (!presented) {
          setPhase((current) => reduceLauncherPhase(current, "present-failed"));
          return;
        }
        window.requestAnimationFrame(() => {
          if (cancelled) return;
          setPhase((current) => reduceLauncherPhase(current, "presented"));
          focusSearch();
        });
      })
      .catch(() => {
        if (!cancelled) setPhase((current) => reduceLauncherPhase(current, "present-failed"));
      });
    return () => {
      cancelled = true;
    };
  }, [focusSearch, phase]);

  // Open settings action from the palette.
  useEffect(() => {
    const open = () => setOpenSettings(true);
    document.addEventListener("prism:open-settings", open);
    return () => document.removeEventListener("prism:open-settings", open);
  }, [setOpenSettings]);

  return (
    <div
      aria-hidden={phase === "hidden" || phase === "closing"}
      className={`launcher-stage launcher-stage-${phase} absolute inset-0`}
    >
      <div
        className="relative h-full w-full max-w-full"
        style={inTauri ? undefined : { width: app.settings.width / (app.settings.viewZoom / 100) }}
      >
        <Palette />
        <SettingsSheet />
        <ToastStack />
      </div>
    </div>
  );
}

export default function App() {
  return (
    <AppProvider>
      <PaletteProvider>
        <Launcher />
      </PaletteProvider>
    </AppProvider>
  );
}
