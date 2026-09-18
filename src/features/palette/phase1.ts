import {
  AppWindow,
  Bluetooth,
  HardDrive,
  Home,
  Lock,
  type LucideIcon,
  Monitor,
  Moon,
  Power,
  RefreshCw,
  RotateCcw,
  Sun,
  Volume2,
  VolumeX,
  Wifi,
} from "lucide-react";
import { executeAction, focusWindow, launchApp, launchAppAsAdmin } from "../../lib/bridge";
import type { Phase1Hit } from "../../lib/query";
import type { AppEntry, PaletteItem, TileTint } from "../../lib/types";
import { isElevatablePath } from "../../lib/types";

const ICONS: Record<string, { icon: LucideIcon; tint: TileTint }> = {
  nightlight: { icon: Moon, tint: "amber" },
  display: { icon: Monitor, tint: "azure" },
  hdr: { icon: Sun, tint: "amber" },
  bluetooth: { icon: Bluetooth, tint: "azure" },
  sound: { icon: Volume2, tint: "mint" },
  storage: { icon: HardDrive, tint: "slate" },
  power: { icon: Power, tint: "rose" },
  update: { icon: RefreshCw, tint: "azure" },
  network: { icon: Wifi, tint: "azure" },
  mute: { icon: VolumeX, tint: "rose" },
  unmute: { icon: Volume2, tint: "mint" },
  lock: { icon: Lock, tint: "slate" },
  sleep: { icon: Moon, tint: "iris" },
  shutdown: { icon: Power, tint: "rose" },
  restart: { icon: RotateCcw, tint: "amber" },
  window: { icon: AppWindow, tint: "azure" },
  settings: { icon: Home, tint: "azure" },
};

export function actionPaletteItem(hit: Phase1Hit): PaletteItem {
  const { icon, tint } = ICONS[hit.iconKey ?? "settings"] ?? ICONS.settings;
  const actionId = hit.actionId ?? hit.id.replace(/^action::/, "");
  return {
    id: hit.id,
    title: hit.title,
    subtitle: hit.subtitle,
    icon: { kind: "tile", icon, tint },
    historyTitle: hit.title,
    run: () => executeAction(actionId),
  };
}

export function windowPaletteItem(hit: Phase1Hit): PaletteItem {
  const hwnd = hit.hwnd;
  const { icon, tint } = ICONS.window;
  return {
    id: hit.id,
    title: hit.title,
    subtitle: hit.subtitle,
    icon: { kind: "tile", icon, tint },
    historyTitle: hit.title,
    run: () => (hwnd == null ? Promise.resolve() : focusWindow(hwnd)),
  };
}

export function appHitPaletteItem(
  hit: Phase1Hit,
  appsById: ReadonlyMap<string, AppEntry>,
  icons: Readonly<Record<string, string>>,
  toItem: (app: AppEntry) => PaletteItem,
): PaletteItem | null {
  const appId = hit.appId;
  if (!appId) return null;
  const entry = appsById.get(appId);
  if (entry) return toItem(entry);
  return {
    id: hit.id,
    title: hit.title,
    subtitle: hit.subtitle,
    icon: { kind: "app", name: hit.title, icon: icons[appId] },
    historyTitle: hit.title,
    appId,
    run: () => launchApp(appId),
    runAsAdmin: isElevatablePath(hit.path) ? () => launchAppAsAdmin(appId) : undefined,
  };
}

export function isCommandAction(hit: Phase1Hit): boolean {
  return hit.kind === "action" && !hit.uri;
}
