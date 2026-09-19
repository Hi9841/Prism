export const LAUNCHER_MOTION_MS = 110;
export const POPOVER_MOTION_MS = 110;

/** Must stay <= PALETTE_HIDE_DELAY in src-tauri/src/lib.rs (125ms). */
export type LauncherPhase = "hidden" | "preparing" | "visible" | "closing";

export type LauncherEvent =
  | "open"
  | "native-close"
  | "web-close"
  | "presented"
  | "present-failed"
  | "exit-finished";

export type HideInvocation = "none" | "instant" | "deferred";

export function reduceLauncherPhase(phase: LauncherPhase, event: LauncherEvent): LauncherPhase {
  switch (event) {
    case "open":
      return phase === "closing" || phase === "visible" ? "visible" : "preparing";
    case "presented":
      return phase === "preparing" ? "visible" : phase;
    case "present-failed":
      return phase === "preparing" || phase === "visible" ? "hidden" : phase;
    case "native-close":
    case "web-close":
      if (phase === "hidden" || phase === "closing") return phase;
      if (phase === "preparing") return "hidden";
      return "closing";
    case "exit-finished":
      return phase === "closing" ? "hidden" : phase;
  }
}

export function hidePaletteInvocation(
  origin: "native" | "web",
  nextPhase: LauncherPhase,
): HideInvocation {
  if (origin === "native") return "none";
  if (nextPhase === "hidden") return "instant";
  if (nextPhase === "closing") return "deferred";
  return "none";
}
