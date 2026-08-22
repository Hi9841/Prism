import {
  Accessibility,
  Bluetooth,
  Clock,
  Gamepad2,
  Home,
  LayoutGrid,
  type LucideIcon,
  Monitor,
  Paintbrush,
  RefreshCw,
  Shield,
  UserRound,
  Wifi,
} from "lucide-react";
import { openWindowsSettings } from "../../lib/bridge";
import { fuzzyScore } from "../../lib/search";
import type { PaletteItem } from "../../lib/types";

interface WindowsSettingsPage {
  title: string;
  uri: string;
  keywords: string[];
  icon: LucideIcon;
}

type WindowsSettingsPageSeed = Omit<WindowsSettingsPage, "icon">;

const SEARCH_SETTINGS_LIMIT = 6;

function settingsPages(icon: LucideIcon, pages: WindowsSettingsPageSeed[]): WindowsSettingsPage[] {
  return pages.map((page) => ({ ...page, icon }));
}

/** Static destinations documented by Microsoft for Windows 10 and 11. */
export const WINDOWS_SETTINGS_PAGES: WindowsSettingsPage[] = [
  ...settingsPages(Home, [
    { title: "Settings", uri: "ms-settings:", keywords: ["windows settings", "system settings"] },
  ]),
  ...settingsPages(Monitor, [
    {
      title: "Display",
      uri: "ms-settings:display",
      keywords: ["display settings", "screen", "monitor", "resolution"],
    },
    {
      title: "Advanced display",
      uri: "ms-settings:display-advanced",
      keywords: ["refresh rate", "display information"],
    },
    { title: "Sound", uri: "ms-settings:sound", keywords: ["audio", "speakers", "microphone"] },
    {
      title: "Sound devices",
      uri: "ms-settings:sound-devices",
      keywords: ["audio devices", "input", "output"],
    },
    { title: "Volume mixer", uri: "ms-settings:apps-volume", keywords: ["app volume", "sound mixer"] },
    {
      title: "Notifications",
      uri: "ms-settings:notifications",
      keywords: ["notification settings", "alerts"],
    },
    {
      title: "Focus assist",
      uri: "ms-settings:quiethours",
      keywords: ["focus", "do not disturb", "quiet hours"],
    },
    {
      title: "Power & sleep",
      uri: "ms-settings:powersleep",
      keywords: ["power", "battery", "sleep settings"],
    },
    { title: "Storage", uri: "ms-settings:storagesense", keywords: ["disk space", "storage settings"] },
    {
      title: "Storage Sense",
      uri: "ms-settings:storagepolicies",
      keywords: ["automatic cleanup", "temporary files"],
    },
    { title: "Clipboard", uri: "ms-settings:clipboard", keywords: ["clipboard history", "copy paste"] },
    { title: "Multitasking", uri: "ms-settings:multitasking", keywords: ["snap windows", "desktops"] },
    { title: "Remote Desktop", uri: "ms-settings:remotedesktop", keywords: ["remote access", "rdp"] },
    {
      title: "About",
      uri: "ms-settings:about",
      keywords: ["system information", "device specifications", "pc info"],
    },
  ]),
  ...settingsPages(Bluetooth, [
    {
      title: "Bluetooth",
      uri: "ms-settings:bluetooth",
      keywords: ["bluetooth settings", "pair device", "wireless devices"],
    },
    {
      title: "Printers & scanners",
      uri: "ms-settings:printers",
      keywords: ["printer", "scanner", "printing"],
    },
    {
      title: "Mouse & touchpad",
      uri: "ms-settings:mousetouchpad",
      keywords: ["mouse settings", "pointer", "trackpad"],
    },
    { title: "Touchpad", uri: "ms-settings:devices-touchpad", keywords: ["trackpad", "touchpad gestures"] },
    {
      title: "Typing",
      uri: "ms-settings:typing",
      keywords: ["keyboard typing", "text suggestions", "autocorrect"],
    },
    { title: "USB", uri: "ms-settings:usb", keywords: ["usb devices", "connection notifications"] },
    { title: "Camera settings", uri: "ms-settings:camera", keywords: ["webcam", "camera device"] },
  ]),
  ...settingsPages(Wifi, [
    {
      title: "Network & internet",
      uri: "ms-settings:network-status",
      keywords: ["network status", "internet settings"],
    },
    { title: "Wi-Fi", uri: "ms-settings:network-wifi", keywords: ["wifi", "wireless network"] },
    { title: "Ethernet", uri: "ms-settings:network-ethernet", keywords: ["wired network", "lan"] },
    { title: "VPN", uri: "ms-settings:network-vpn", keywords: ["virtual private network"] },
    {
      title: "Mobile hotspot",
      uri: "ms-settings:network-mobilehotspot",
      keywords: ["hotspot", "internet sharing"],
    },
    {
      title: "Airplane mode",
      uri: "ms-settings:network-airplanemode",
      keywords: ["flight mode", "wireless off"],
    },
    { title: "Proxy", uri: "ms-settings:network-proxy", keywords: ["proxy server", "network proxy"] },
  ]),
  ...settingsPages(Paintbrush, [
    {
      title: "Background",
      uri: "ms-settings:personalization-background",
      keywords: ["wallpaper", "desktop background"],
    },
    {
      title: "Colors",
      uri: "ms-settings:personalization-colors",
      keywords: ["accent color", "dark mode", "light mode"],
    },
    { title: "Themes", uri: "ms-settings:themes", keywords: ["windows theme", "personalization"] },
    { title: "Lock screen", uri: "ms-settings:lockscreen", keywords: ["lockscreen", "screen timeout"] },
    { title: "Start", uri: "ms-settings:personalization-start", keywords: ["start menu", "start settings"] },
    { title: "Taskbar", uri: "ms-settings:taskbar", keywords: ["taskbar settings", "system tray"] },
    { title: "Fonts", uri: "ms-settings:fonts", keywords: ["font settings", "typefaces"] },
  ]),
  ...settingsPages(LayoutGrid, [
    {
      title: "Installed apps",
      uri: "ms-settings:appsfeatures",
      keywords: ["apps and features", "uninstall apps", "programs"],
    },
    {
      title: "Default apps",
      uri: "ms-settings:defaultapps",
      keywords: ["file associations", "default programs", "browser default"],
    },
    {
      title: "Optional features",
      uri: "ms-settings:optionalfeatures",
      keywords: ["windows features", "feature install"],
    },
    {
      title: "Startup apps",
      uri: "ms-settings:startupapps",
      keywords: ["startup programs", "login apps", "boot apps"],
    },
  ]),
  ...settingsPages(UserRound, [
    { title: "Your info", uri: "ms-settings:yourinfo", keywords: ["account info", "profile picture"] },
    {
      title: "Sign-in options",
      uri: "ms-settings:signinoptions",
      keywords: ["password", "pin", "windows hello", "login"],
    },
    {
      title: "Email & app accounts",
      uri: "ms-settings:emailandaccounts",
      keywords: ["email accounts", "app accounts"],
    },
    {
      title: "Access work or school",
      uri: "ms-settings:workplace",
      keywords: ["work account", "school account", "organization"],
    },
    {
      title: "Family & other users",
      uri: "ms-settings:otherusers",
      keywords: ["family", "users", "add account"],
    },
  ]),
  ...settingsPages(Clock, [
    {
      title: "Date & time",
      uri: "ms-settings:dateandtime",
      keywords: ["clock", "time zone", "date settings"],
    },
    {
      title: "Language & region",
      uri: "ms-settings:regionlanguage",
      keywords: ["language", "region", "locale"],
    },
    { title: "Speech", uri: "ms-settings:speech", keywords: ["speech language", "voice recognition"] },
  ]),
  ...settingsPages(Gamepad2, [
    { title: "Game Mode", uri: "ms-settings:gaming-gamemode", keywords: ["gaming mode", "game performance"] },
    { title: "Xbox Game Bar", uri: "ms-settings:gaming-gamebar", keywords: ["game bar", "xbox overlay"] },
  ]),
  ...settingsPages(Accessibility, [
    {
      title: "Narrator",
      uri: "ms-settings:easeofaccess-narrator",
      keywords: ["screen reader", "accessibility narrator"],
    },
    {
      title: "Magnifier",
      uri: "ms-settings:easeofaccess-magnifier",
      keywords: ["screen magnifier", "zoom accessibility"],
    },
    {
      title: "Contrast themes",
      uri: "ms-settings:easeofaccess-highcontrast",
      keywords: ["high contrast", "accessibility colors"],
    },
    {
      title: "Color filters",
      uri: "ms-settings:easeofaccess-colorfilter",
      keywords: ["color blindness", "accessibility filters"],
    },
    {
      title: "Captions",
      uri: "ms-settings:easeofaccess-closedcaptioning",
      keywords: ["closed captions", "subtitles", "accessibility captions"],
    },
  ]),
  ...settingsPages(Shield, [
    {
      title: "Privacy & security",
      uri: "ms-settings:privacy",
      keywords: ["privacy settings", "permissions"],
    },
    {
      title: "Camera privacy",
      uri: "ms-settings:privacy-webcam",
      keywords: ["camera permissions", "webcam privacy"],
    },
    {
      title: "Microphone privacy",
      uri: "ms-settings:privacy-microphone",
      keywords: ["microphone permissions", "mic privacy"],
    },
    {
      title: "Location privacy",
      uri: "ms-settings:privacy-location",
      keywords: ["location permissions", "gps privacy"],
    },
    {
      title: "Search permissions",
      uri: "ms-settings:search-permissions",
      keywords: ["windows search", "search history"],
    },
    {
      title: "Windows Security",
      uri: "ms-settings:windowsdefender",
      keywords: ["defender", "virus protection", "security settings"],
    },
  ]),
  ...settingsPages(RefreshCw, [
    { title: "Windows Update", uri: "ms-settings:windowsupdate", keywords: ["updates", "check for updates"] },
    {
      title: "Update history",
      uri: "ms-settings:windowsupdate-history",
      keywords: ["windows update history", "installed updates"],
    },
    {
      title: "Advanced update options",
      uri: "ms-settings:windowsupdate-options",
      keywords: ["windows update advanced", "update options"],
    },
    {
      title: "Optional updates",
      uri: "ms-settings:windowsupdate-optionalupdates",
      keywords: ["driver updates", "windows optional updates"],
    },
  ]),
  ...settingsPages(Monitor, [
    {
      title: "Activation",
      uri: "ms-settings:activation",
      keywords: ["windows activation", "product key", "license"],
    },
    { title: "Recovery", uri: "ms-settings:recovery", keywords: ["reset pc", "advanced startup", "restore"] },
    { title: "Troubleshoot", uri: "ms-settings:troubleshoot", keywords: ["troubleshooter", "fix problems"] },
    {
      title: "For developers",
      uri: "ms-settings:developers",
      keywords: ["developer mode", "development settings"],
    },
  ]),
  ...settingsPages(Shield, [
    { title: "Find my device", uri: "ms-settings:findmydevice", keywords: ["locate device", "lost pc"] },
  ]),
];

export function searchWindowsSettings(query: string): PaletteItem[] {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return [];

  const ranked = WINDOWS_SETTINGS_PAGES.map((page) => ({ page, score: settingsPageScore(page, normalized) }))
    .filter(
      (candidate): candidate is { page: WindowsSettingsPage; score: number } => candidate.score !== null,
    )
    .sort((a, b) => b.score - a.score);
  const exactTitleMatches = ranked.filter((candidate) => candidate.score === 1_000);

  return (exactTitleMatches.length > 0 ? exactTitleMatches : ranked)
    .slice(0, SEARCH_SETTINGS_LIMIT)
    .map(({ page }) => ({
      id: `settings::${page.uri}`,
      title: page.title,
      subtitle: "Windows Settings",
      keywords: page.keywords,
      icon: { kind: "tile", icon: page.icon, tint: "azure" },
      historyTitle: page.title,
      run: () => openWindowsSettings(page.uri),
    }));
}

function settingsPageScore(page: WindowsSettingsPage, query: string): number | null {
  const title = page.title.toLowerCase();
  if (title === query) return 1_000;

  let score: number | null = null;
  if (title.startsWith(query)) score = 950 + query.length;
  else if (title.includes(query)) score = 850 + query.length;
  else {
    const fuzzyTitleScore = fuzzyScore(query, title);
    const minimumFuzzyScore = Math.max(24, query.replace(/[^a-z0-9]/g, "").length * 6);
    if (fuzzyTitleScore !== null && fuzzyTitleScore >= minimumFuzzyScore) score = fuzzyTitleScore;
  }

  for (const keyword of page.keywords) {
    const normalizedKeyword = keyword.toLowerCase();
    let keywordScore: number | null = null;
    if (normalizedKeyword === query) keywordScore = 900;
    else if (normalizedKeyword.startsWith(query)) keywordScore = 800 + query.length;
    else if (normalizedKeyword.includes(query)) keywordScore = 700 + query.length;
    if (keywordScore !== null && (score === null || keywordScore > score)) score = keywordScore;
  }
  return score;
}
