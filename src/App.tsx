import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { SettingsSheet } from "./components/SettingsSheet";
import { ToastStack } from "./components/Toast";
import { Palette } from "./features/palette/Palette";
import {
  hidePaletteWindow,
  inTauri,
  isWindowVisible,
  onToggleRequest,
  onWindowFocused,
  presentPaletteWindow,
} from "./lib/bridge";
import { AppProvider, useApp } from "./state/app";
import { PaletteProvider, usePalette } from "./state/palette";

function Launcher() {
  const app = useApp();
  const palette = usePalette();
  const { reset } = palette;
  const { setOpenSettings } = app;
  const [phase, setPhase] = useState<"hidden" | "preparing" | "visible">(inTauri ? "hidden" : "visible");
  const visible = phase !== "hidden";
  const visibleRef = useRef(false);
  const blurCheck = useRef<number | null>(null);

  visibleRef.current = visible;

  const hide = useCallback(() => {
    if (!visibleRef.current) return;
    setPhase("hidden");
    hidePaletteWindow().catch(() => {});
  }, []);

  // Rust-side: global hotkey pressed, or user clicked away (blur).
  useEffect(() => {
    const offToggle = onToggleRequest((request) => {
      if (!request.open) {
        hide();
      } else {
        // Fresh state on every open: no leftover query, selection or
        // settings overlay - the palette starts from zero.
        reset();
        setOpenSettings(false);
        setPhase("preparing");
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
              setPhase("hidden");
            }
          })
          .catch(() => {});
      }, 60);
    });
    document.addEventListener("prism:close", hide);
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
      document.removeEventListener("prism:close", hide);
    };
  }, [hide, reset, setOpenSettings]);

  // Commit the fully prepared DOM while the native window is still hidden.
  // Only then present the window and begin the entrance on the next frame.
  useLayoutEffect(() => {
    if (phase !== "preparing") return;
    let cancelled = false;
    presentPaletteWindow()
      .then((presented) => {
        if (cancelled) return;
        if (!presented) {
          setPhase("hidden");
          return;
        }
        window.requestAnimationFrame(() => {
          if (cancelled) return;
          setPhase("visible");
          document.querySelector<HTMLInputElement>("[data-prism-search]")?.focus();
        });
      })
      .catch(() => {
        if (!cancelled) setPhase("hidden");
      });
    return () => {
      cancelled = true;
    };
  }, [phase]);

  // Open settings action from the palette.
  useEffect(() => {
    const open = () => setOpenSettings(true);
    document.addEventListener("prism:open-settings", open);
    return () => document.removeEventListener("prism:open-settings", open);
  }, [setOpenSettings]);

  return (
    <div
      aria-hidden={phase === "hidden"}
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
