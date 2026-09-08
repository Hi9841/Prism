import {
  Check,
  ChevronDown,
  ChevronUp,
  Download,
  FileText,
  FileVideo,
  History,
  Home,
  Images,
  Keyboard,
  LogOut,
  Minus,
  Monitor,
  Music,
  Plus,
  RotateCcw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getAppIcons, getAppVersion, setShortcut, setTaskbarAlignment } from "../lib/bridge";
import type {
  AccentId,
  AppGroup,
  QuickAccessKind,
  TaskbarAlignment,
  ThemeMode,
  WindowWidth,
} from "../lib/types";
import {
  APP_GROUP_APP_LIMIT,
  APP_GROUP_LIMIT,
  DEFAULT_SETTINGS,
  QUICK_ACCESS_LIMIT,
  stepViewZoom,
  VIEW_ZOOM_LEVELS,
} from "../lib/types";
import { SHORTCUT_OPTIONS, THEME_OPTIONS, useApp } from "../state/app";
import { usePalette } from "../state/palette";
import { TaskbarCustomization } from "./TaskbarCustomization";
import { RowIcon, Segmented, SettingsRow, Toggle } from "./ui";

const ACCENTS: { id: AccentId; name: string }[] = [
  { id: "iris", name: "Iris" },
  { id: "azure", name: "Azure" },
  { id: "mint", name: "Mint" },
  { id: "amber", name: "Amber" },
  { id: "rose", name: "Rose" },
];

/** "Ctrl+Alt+Space" -> "Ctrl + Alt + Space" */
export function displayShortcut(combo: string): string {
  return combo.replace(/\+/g, " + ");
}

function KeybindPicker() {
  const { settings, updateSettings, showToast } = useApp();
  const [busy, setBusy] = useState(false);

  const applyShortcut = useCallback(
    async (combo: string) => {
      if (combo === settings.shortcut || busy) return;
      setBusy(true);
      try {
        // OS first: the backend validates, registers, and only then
        // releases the previous binding. State persists only on success.
        await setShortcut(combo);
        updateSettings({ shortcut: combo });
        showToast("Shortcut set", displayShortcut(combo));
      } catch (e) {
        showToast("Shortcut not changed", String(e));
      } finally {
        setBusy(false);
      }
    },
    [settings.shortcut, busy, updateSettings, showToast],
  );

  return (
    <div className="flex w-full min-w-0 flex-col items-end gap-2">
      <div className="flex items-center gap-2">
        <span className="text-[12px] font-medium text-fg">{displayShortcut(settings.shortcut)}</span>
        <Keyboard className="h-3.5 w-3.5 text-fg-tertiary" />
      </div>
      <div className="flex flex-wrap justify-end gap-1.5">
        {SHORTCUT_OPTIONS.map((o) => {
          const active = settings.shortcut === o.value;
          return (
            <button
              key={o.value}
              type="button"
              aria-pressed={active}
              disabled={busy}
              onClick={() => applyShortcut(o.value)}
              className={`focus-ring press cursor-pointer rounded-[9px] px-2 py-[4px] text-[11px] font-medium ${
                active
                  ? "bg-accent-soft text-fg"
                  : "bg-surface text-fg-tertiary hover:bg-surface-hover hover:text-fg-secondary"
              } ${busy ? "cursor-default opacity-50" : ""}`}
            >
              {o.label}
            </button>
          );
        })}
      </div>
      {settings.shortcut === "Win" && (
        <p className="max-w-[250px] text-right text-[11px] leading-relaxed text-fg-tertiary">
          Standalone Win opens only Prism, including with replacement Start menus. Win + key shortcuts keep
          working.
        </p>
      )}
    </div>
  );
}

function TaskbarAlignmentPicker() {
  const { settings, updateSettings, showToast } = useApp();
  const [busy, setBusy] = useState(false);

  const applyAlignment = useCallback(
    async (taskbarAlignment: TaskbarAlignment) => {
      if (taskbarAlignment === settings.taskbarAlignment || busy) return;
      setBusy(true);
      try {
        await setTaskbarAlignment(taskbarAlignment);
        updateSettings({ taskbarAlignment });
      } catch (error) {
        showToast("Taskbar not changed", String(error));
      } finally {
        setBusy(false);
      }
    },
    [busy, settings.taskbarAlignment, showToast, updateSettings],
  );

  return (
    <fieldset disabled={busy} className="m-0 min-w-0 border-0 p-0 disabled:opacity-55">
      <Segmented<TaskbarAlignment>
        label="Taskbar alignment"
        value={settings.taskbarAlignment}
        onChange={applyAlignment}
        options={[
          { value: "left", label: "Left" },
          { value: "center", label: "Center" },
          { value: "right", label: "Right" },
        ]}
      />
      <p className="mt-1.5 max-w-[250px] text-right text-[11px] leading-relaxed text-fg-tertiary">
        Right alignment requires the StartAllBack classic taskbar.
      </p>
    </fieldset>
  );
}

function ViewZoomControl() {
  const { settings, updateSettings } = useApp();
  const atMinimum = settings.viewZoom === VIEW_ZOOM_LEVELS[0];
  const atMaximum = settings.viewZoom === VIEW_ZOOM_LEVELS[VIEW_ZOOM_LEVELS.length - 1];

  const adjust = (direction: -1 | 1) => {
    updateSettings({ viewZoom: stepViewZoom(settings.viewZoom, direction) });
  };

  return (
    <fieldset
      aria-label="View zoom"
      className="m-0 flex items-center gap-1 rounded-[10px] border-0 bg-surface p-[3px]"
    >
      <button
        type="button"
        aria-label="Zoom out"
        title="Zoom out"
        disabled={atMinimum}
        onClick={() => adjust(-1)}
        className="focus-ring press grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-tertiary hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-35"
      >
        <Minus className="h-3.5 w-3.5" />
      </button>
      <output aria-live="polite" className="w-11 text-center text-[12px] font-medium text-fg tabular-nums">
        {settings.viewZoom}%
      </output>
      <button
        type="button"
        aria-label="Zoom in"
        title="Zoom in"
        disabled={atMaximum}
        onClick={() => adjust(1)}
        className="focus-ring press grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-tertiary hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-35"
      >
        <Plus className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        aria-label="Reset zoom"
        title="Reset zoom"
        disabled={settings.viewZoom === DEFAULT_SETTINGS.viewZoom}
        onClick={() => updateSettings({ viewZoom: DEFAULT_SETTINGS.viewZoom })}
        className="focus-ring press grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-tertiary hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-35"
      >
        <RotateCcw className="h-3.5 w-3.5" />
      </button>
    </fieldset>
  );
}

const QUICK_ACCESS_OPTIONS: {
  kind: QuickAccessKind;
  label: string;
  icon: typeof Home;
}[] = [
  { kind: "home", label: "Home", icon: Home },
  { kind: "desktop", label: "Desktop", icon: Monitor },
  { kind: "downloads", label: "Downloads", icon: Download },
  { kind: "documents", label: "Documents", icon: FileText },
  { kind: "pictures", label: "Pictures", icon: Images },
  { kind: "music", label: "Music", icon: Music },
  { kind: "videos", label: "Videos", icon: FileVideo },
];

function QuickAccessPicker() {
  const { settings, updateSettings } = useApp();

  const toggle = (kind: QuickAccessKind) => {
    const active = settings.quickAccess.includes(kind);
    const quickAccess = active
      ? settings.quickAccess.filter((entry) => entry !== kind)
      : [...settings.quickAccess, kind];
    updateSettings({ quickAccess });
  };

  return (
    <fieldset
      aria-label="Pinned Quick Access folders"
      className="grid w-[198px] grid-cols-2 gap-1.5 border-0 p-0"
    >
      {QUICK_ACCESS_OPTIONS.map(({ kind, label, icon: FolderIcon }) => {
        const active = settings.quickAccess.includes(kind);
        const disabled = !active && settings.quickAccess.length >= QUICK_ACCESS_LIMIT;
        return (
          <button
            key={kind}
            type="button"
            aria-pressed={active}
            disabled={disabled}
            onClick={() => toggle(kind)}
            className={`focus-ring press flex h-8 min-w-0 cursor-pointer items-center gap-1.5 rounded-[8px] px-2 text-[11px] font-medium disabled:cursor-default disabled:opacity-35 ${
              active
                ? "bg-accent-soft text-fg"
                : "bg-surface text-fg-tertiary hover:bg-surface-hover hover:text-fg-secondary"
            }`}
          >
            <FolderIcon className={`h-3.5 w-3.5 shrink-0 ${active ? "text-accent" : ""}`} />
            <span className="truncate">{label}</span>
          </button>
        );
      })}
    </fieldset>
  );
}

function AppGroupsPicker() {
  const { settings, updateSettings, showToast } = useApp();
  const { apps, appsLoaded } = usePalette();
  const [newName, setNewName] = useState("");
  const [openGroupId, setOpenGroupId] = useState<string | null>(null);
  const [appQuery, setAppQuery] = useState("");
  const [pickerIcons, setPickerIcons] = useState<Record<string, string>>({});
  const appSearchRef = useRef<HTMLInputElement>(null);
  const appPickerRef = useRef<HTMLDivElement>(null);
  const requestedPickerIconsRef = useRef(new Set<string>());
  const availableApps = useMemo(
    () => [...apps].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true })),
    [apps],
  );
  const filteredApps = useMemo(() => {
    const normalized = appQuery.trim().toLowerCase();
    return normalized
      ? availableApps.filter((entry) => entry.name.toLowerCase().includes(normalized))
      : availableApps;
  }, [appQuery, availableApps]);

  useEffect(() => {
    if (!openGroupId) return;
    appSearchRef.current?.focus();
    const closeOnOutside = (event: PointerEvent) => {
      if (!appPickerRef.current?.contains(event.target as Node)) setOpenGroupId(null);
    };
    document.addEventListener("pointerdown", closeOnOutside);
    return () => document.removeEventListener("pointerdown", closeOnOutside);
  }, [openGroupId]);

  useEffect(() => {
    if (!openGroupId) return;
    const missingIds = filteredApps
      .slice(0, 512)
      .map((entry) => entry.appId)
      .filter((appId) => !pickerIcons[appId] && !requestedPickerIconsRef.current.has(appId));
    if (missingIds.length === 0) return;

    for (const appId of missingIds) requestedPickerIconsRef.current.add(appId);
    void getAppIcons(missingIds)
      .then((icons) => {
        if (Object.keys(icons).length > 0) {
          setPickerIcons((current) => ({ ...current, ...icons }));
        }
      })
      .catch(() => {
        for (const appId of missingIds) requestedPickerIconsRef.current.delete(appId);
      });
  }, [filteredApps, openGroupId, pickerIcons]);

  const toggleAppPicker = (groupId: string) => {
    setOpenGroupId((current) => (current === groupId ? null : groupId));
    setAppQuery("");
  };

  const createGroup = () => {
    const name = newName.trim();
    if (!name) return;
    if (settings.appGroups.length >= APP_GROUP_LIMIT) {
      showToast("Collection limit reached", `Prism supports up to ${APP_GROUP_LIMIT} collections`);
      return;
    }
    const id = `group-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
    updateSettings({ appGroups: [...settings.appGroups, { id, name, appIds: [], collapsed: false }] });
    setNewName("");
  };

  const updateGroup = (id: string, patch: Partial<AppGroup>) => {
    updateSettings({
      appGroups: settings.appGroups.map((group) => (group.id === id ? { ...group, ...patch } : group)),
    });
  };

  const removeGroup = (id: string) => {
    updateSettings({ appGroups: settings.appGroups.filter((group) => group.id !== id) });
  };

  const moveGroup = (id: string, direction: -1 | 1) => {
    const index = settings.appGroups.findIndex((group) => group.id === id);
    const target = index + direction;
    if (index < 0 || target < 0 || target >= settings.appGroups.length) return;
    const appGroups = [...settings.appGroups];
    const [group] = appGroups.splice(index, 1);
    appGroups.splice(target, 0, group);
    updateSettings({ appGroups });
  };

  const assignApp = (groupId: string, appId: string) => {
    if (!appId) return;
    const target = settings.appGroups.find((group) => group.id === groupId);
    if (!target || target.appIds.includes(appId)) return;
    if (target.appIds.length >= APP_GROUP_APP_LIMIT) {
      showToast("Collection is full", `Each collection supports up to ${APP_GROUP_APP_LIMIT} apps`);
      return;
    }
    updateSettings({
      appGroups: settings.appGroups.map((group) => ({
        ...group,
        appIds:
          group.id === groupId
            ? [...group.appIds, appId]
            : group.appIds.filter((candidate) => candidate !== appId),
      })),
    });
  };

  const removeApp = (groupId: string, appId: string) => {
    updateGroup(groupId, {
      appIds:
        settings.appGroups.find((group) => group.id === groupId)?.appIds.filter((id) => id !== appId) ?? [],
    });
  };

  return (
    <div className="w-[258px] min-w-0 space-y-2">
      <div className="flex gap-1.5">
        <input
          value={newName}
          onChange={(event) => setNewName(event.target.value)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === "Enter") createGroup();
          }}
          maxLength={64}
          placeholder="New collection"
          aria-label="New app collection name"
          className="focus-ring min-w-0 flex-1 rounded-[8px] bg-surface px-2.5 py-1.5 text-[12px] text-fg outline-none placeholder:text-fg-quiet"
        />
        <button
          type="button"
          aria-label="Create app collection"
          title="Create app collection"
          onClick={createGroup}
          className="focus-ring press grid h-8 w-8 shrink-0 cursor-pointer place-items-center rounded-[8px] bg-accent-soft text-accent hover:bg-surface-hover"
        >
          <Plus className="h-4 w-4" />
        </button>
      </div>
      {settings.appGroups.map((group) => {
        const assigned = group.appIds
          .map((appId) => apps.find((entry) => entry.appId === appId))
          .filter((entry): entry is (typeof apps)[number] => Boolean(entry));
        const assignedIds = new Set(group.appIds);
        return (
          <div key={group.id} className="border-t border-line pt-2">
            <div className="flex items-center gap-1.5">
              <input
                value={group.name}
                maxLength={64}
                aria-label={`${group.name} collection name`}
                onChange={(event) => updateGroup(group.id, { name: event.target.value })}
                className="focus-ring min-w-0 flex-1 bg-transparent text-[12px] font-semibold text-fg outline-none"
              />
              <div className="flex shrink-0 items-center gap-0.5">
                <button
                  type="button"
                  aria-label={`Move ${group.name} up`}
                  title="Move up"
                  disabled={settings.appGroups[0]?.id === group.id}
                  onClick={() => moveGroup(group.id, -1)}
                  className="focus-ring press grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-quiet hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-25"
                >
                  <ChevronUp className="h-3.5 w-3.5" />
                </button>
                <button
                  type="button"
                  aria-label={`Move ${group.name} down`}
                  title="Move down"
                  disabled={settings.appGroups[settings.appGroups.length - 1]?.id === group.id}
                  onClick={() => moveGroup(group.id, 1)}
                  className="focus-ring press grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-quiet hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-25"
                >
                  <ChevronDown className="h-3.5 w-3.5" />
                </button>
                <button
                  type="button"
                  aria-label={`Delete ${group.name} collection`}
                  title="Delete collection"
                  onClick={() => removeGroup(group.id)}
                  className="focus-ring press grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-quiet hover:bg-danger-soft hover:text-danger"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
            <div ref={openGroupId === group.id ? appPickerRef : undefined} className="mt-1.5">
              <button
                type="button"
                aria-label={`Add app to ${group.name}`}
                aria-haspopup="listbox"
                aria-expanded={openGroupId === group.id}
                aria-controls={`app-picker-${group.id}`}
                onClick={() => toggleAppPicker(group.id)}
                className={`focus-ring flex h-9 w-full cursor-pointer items-center gap-2 rounded-[8px] px-2.5 text-left text-[11.5px] font-medium transition-colors duration-150 ${
                  openGroupId === group.id
                    ? "bg-surface-active text-fg shadow-[inset_0_0_0_1px_var(--accent-ring)]"
                    : "bg-surface text-fg-secondary hover:bg-surface-hover hover:text-fg"
                }`}
              >
                <Plus className="h-3.5 w-3.5 shrink-0 text-accent" />
                <span className="min-w-0 flex-1 truncate">
                  {openGroupId === group.id ? "Choose apps" : "Add apps"}
                </span>
                {group.appIds.length > 0 ? (
                  <span className="shrink-0 text-[10.5px] font-semibold text-fg-tertiary tabular-nums">
                    {group.appIds.length}
                  </span>
                ) : null}
                <ChevronDown
                  className={`h-3.5 w-3.5 shrink-0 text-fg-quiet transition-transform duration-150 ${
                    openGroupId === group.id ? "rotate-180" : ""
                  }`}
                />
              </button>
              {openGroupId === group.id ? (
                <div
                  id={`app-picker-${group.id}`}
                  className="mt-1.5 overflow-hidden rounded-[9px] bg-surface shadow-[inset_0_0_0_1px_var(--t-line)]"
                >
                  <div className="m-1.5 flex h-8 items-center gap-2 rounded-[7px] bg-[var(--t-field-bg)] px-2.5 shadow-[inset_0_0_0_1px_var(--t-field-line)] focus-within:shadow-[inset_0_0_0_1px_var(--accent-ring)]">
                    <Search className="h-3.5 w-3.5 shrink-0 text-fg-quiet" />
                    <input
                      ref={appSearchRef}
                      value={appQuery}
                      onChange={(event) => setAppQuery(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.nativeEvent.isComposing) return;
                        if (event.key === "Escape") {
                          event.preventDefault();
                          event.stopPropagation();
                          setOpenGroupId(null);
                        }
                      }}
                      placeholder="Filter apps"
                      aria-label={`Filter apps for ${group.name}`}
                      className="min-w-0 flex-1 bg-transparent text-[11.5px] text-fg outline-none placeholder:text-fg-quiet"
                    />
                    {appQuery ? (
                      <button
                        type="button"
                        aria-label="Clear app filter"
                        title="Clear filter"
                        onClick={() => {
                          setAppQuery("");
                          appSearchRef.current?.focus();
                        }}
                        className="focus-ring grid h-6 w-6 shrink-0 cursor-pointer place-items-center rounded-[6px] text-fg-quiet hover:bg-surface-hover hover:text-fg"
                      >
                        <X className="h-3.5 w-3.5" />
                      </button>
                    ) : null}
                  </div>
                  <div
                    role="listbox"
                    aria-multiselectable="true"
                    aria-label={`Apps available for ${group.name}`}
                    className="scroll-thin max-h-56 overflow-y-auto px-1.5 pb-1.5"
                  >
                    {!appsLoaded ? (
                      <div className="px-2 py-5 text-center text-[11px] text-fg-quiet">
                        Loading installed apps...
                      </div>
                    ) : filteredApps.length === 0 ? (
                      <div className="px-2 py-5 text-center text-[11px] text-fg-quiet">No matching apps</div>
                    ) : (
                      filteredApps.map((entry) => {
                        const owner = settings.appGroups.find((candidate) =>
                          candidate.appIds.includes(entry.appId),
                        );
                        const selected = assignedIds.has(entry.appId);
                        const disabled = !selected && group.appIds.length >= APP_GROUP_APP_LIMIT;
                        return (
                          <button
                            key={entry.appId}
                            type="button"
                            role="option"
                            aria-selected={selected}
                            disabled={disabled}
                            title={disabled ? `${group.name} is full` : undefined}
                            onClick={() =>
                              selected ? removeApp(group.id, entry.appId) : assignApp(group.id, entry.appId)
                            }
                            className={`focus-ring flex min-h-10 w-full cursor-pointer items-center gap-2 rounded-[7px] px-2 py-1 text-left transition-colors duration-100 disabled:cursor-default disabled:opacity-35 ${
                              selected
                                ? "bg-accent-soft text-fg"
                                : "text-fg-secondary hover:bg-surface-hover hover:text-fg"
                            }`}
                          >
                            <RowIcon
                              icon={{
                                kind: "app",
                                name: entry.name,
                                icon: pickerIcons[entry.appId],
                              }}
                              size={26}
                            />
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-[11.5px] font-medium">{entry.name}</span>
                              {owner && !selected ? (
                                <span className="mt-0.5 block truncate text-[10px] text-fg-quiet">
                                  In {owner.name}
                                </span>
                              ) : null}
                            </span>
                            <span
                              className={`grid h-5 w-5 shrink-0 place-items-center rounded-[6px] ${
                                selected ? "bg-accent text-accent-fg" : "bg-surface text-transparent"
                              }`}
                              aria-hidden="true"
                            >
                              <Check className="h-3.5 w-3.5" strokeWidth={2.5} />
                            </span>
                          </button>
                        );
                      })
                    )}
                  </div>
                  <div className="flex items-center justify-between border-t border-line px-2.5 py-2">
                    <span className="text-[10.5px] text-fg-quiet tabular-nums">
                      {group.appIds.length} of {APP_GROUP_APP_LIMIT} selected
                    </span>
                    <button
                      type="button"
                      onClick={() => setOpenGroupId(null)}
                      className="focus-ring press cursor-pointer rounded-[7px] bg-accent-soft px-2.5 py-1 text-[11px] font-semibold text-fg hover:bg-surface-hover"
                    >
                      Done
                    </button>
                  </div>
                </div>
              ) : null}
            </div>
            {assigned.length > 0 && openGroupId !== group.id ? (
              <div className="mt-1.5 flex flex-wrap gap-1">
                {assigned.map((entry) => (
                  <button
                    key={entry.appId}
                    type="button"
                    aria-label={`Remove ${entry.name} from ${group.name}`}
                    title={`Remove ${entry.name}`}
                    onClick={() => removeApp(group.id, entry.appId)}
                    className="focus-ring press flex max-w-full cursor-pointer items-center gap-1 rounded-[6px] bg-accent-soft px-2 py-1 text-[10.5px] font-medium text-fg hover:bg-surface-hover"
                  >
                    <span className="truncate">{entry.name}</span>
                    <X className="h-3 w-3 shrink-0 text-fg-quiet" aria-hidden="true" />
                  </button>
                ))}
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

export function SettingsSheet() {
  const {
    settings,
    updateSettings,
    resetSettings,
    persistenceError,
    retryPersistence,
    quit,
    openSettings,
    setOpenSettings,
    clearHistory,
    showToast,
  } = useApp();
  const panelRef = useRef<HTMLDivElement>(null);
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [closing, setClosing] = useState(false);
  const [titleId] = useState(() => `prism-settings-title-${Math.random().toString(36).slice(2, 8)}`);
  const [version, setVersion] = useState("");
  const [resetConfirming, setResetConfirming] = useState(false);
  const [resetBusy, setResetBusy] = useState(false);
  const [retrySaveBusy, setRetrySaveBusy] = useState(false);

  useEffect(() => {
    void getAppVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  // Deliberate closes (X, backdrop) animate the sheet out first; Esc snaps.
  const requestClose = useCallback(() => {
    if (closing) return;
    setClosing(true);
    closeTimerRef.current = setTimeout(() => {
      closeTimerRef.current = null;
      setOpenSettings(false);
    }, 150);
  }, [closing, setOpenSettings]);

  // A fresh open resets the exit state and cancels any pending close.
  useEffect(() => {
    if (!openSettings) return;
    if (closeTimerRef.current) {
      clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
    setClosing(false);
  }, [openSettings]);

  useEffect(() => {
    if (!openSettings) setResetConfirming(false);
  }, [openSettings]);

  const confirmReset = useCallback(async () => {
    if (resetBusy) return;
    if (!resetConfirming) {
      setResetConfirming(true);
      return;
    }
    setResetBusy(true);
    try {
      await resetSettings();
      setResetConfirming(false);
      showToast("Settings reset", "Prism is back to its defaults");
    } catch (error) {
      showToast("Settings not reset", String(error));
    } finally {
      setResetBusy(false);
    }
  }, [resetBusy, resetConfirming, resetSettings, showToast]);

  const retrySave = useCallback(async () => {
    if (retrySaveBusy) return;
    setRetrySaveBusy(true);
    try {
      await retryPersistence();
    } catch (error) {
      showToast("Settings still not saved", String(error), "error");
    } finally {
      setRetrySaveBusy(false);
    }
  }, [retryPersistence, retrySaveBusy, showToast]);

  useEffect(
    () => () => {
      if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
    },
    [],
  );

  // Focus management: focus the panel on open, restore it on close.
  useEffect(() => {
    if (!openSettings) return;
    const panel = panelRef.current;
    const first = panel?.querySelector<HTMLElement>("[data-autofocus], button, [role='switch']");
    first?.focus();
    return () => {
      // Restore focus to the settings trigger in the palette header.
      document.querySelector<HTMLElement>("[data-settings-trigger]")?.focus();
    };
  }, [openSettings]);

  // Focus trap: Tab/Shift+Tab cycle within the sheet.
  const onPanelKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.nativeEvent.isComposing) return;
      if (e.key === "Escape") {
        e.preventDefault();
        setOpenSettings(false);
        return;
      }
      if (e.key !== "Tab") return;
      const panel = panelRef.current;
      if (!panel) return;
      const focusables = Array.from(
        panel.querySelectorAll<HTMLElement>(
          "button:not([disabled]), [role='switch'], input, select, textarea, [tabindex]:not([tabindex='-1'])",
        ),
      ).filter((el) => el.offsetParent !== null && el.tabIndex >= 0);
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    },
    [setOpenSettings],
  );

  if (!openSettings) return null;

  return (
    <div
      className={`settings-backdrop ${closing ? "settings-backdrop-exit" : ""} absolute inset-0 z-40 flex`}
    >
      <button
        type="button"
        aria-label="Close settings"
        className="absolute inset-0 cursor-default rounded-[24px_24px_8px_8px] bg-backdrop backdrop-blur-[2px]"
        onClick={() => requestClose()}
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onPanelKeyDown}
        className={`settings-panel ${closing ? "settings-panel-exit" : ""} relative ml-auto flex w-[min(340px,88%)] flex-col overflow-hidden rounded-tr-[24px] rounded-br-[8px] border-l border-line bg-bg-raised backdrop-blur-2xl`}
        style={{ boxShadow: "-24px 0 64px rgb(0 0 0 / 0.35)" }}
      >
        <div className="flex items-center justify-between px-5 pb-2 pt-5">
          <h2 id={titleId} className="text-balance text-[15px] font-semibold text-fg">
            Settings
          </h2>
          <button
            type="button"
            aria-label="Close settings"
            onClick={() => requestClose()}
            className="focus-ring press relative grid cursor-pointer place-items-center rounded-[6px] text-fg-tertiary hover:bg-surface-hover hover:text-fg-secondary"
            style={{ width: 44, height: 44 }}
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="mx-5 mb-1 border-b border-line pb-3">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="text-[13px] font-semibold text-fg">Reset all settings</div>
              <div className="mt-0.5 text-[11.5px] leading-relaxed text-fg-tertiary">
                Restore Prism&apos;s appearance, behavior, pins, and collections.
              </div>
            </div>
            <button
              type="button"
              disabled={resetBusy}
              onClick={() => void confirmReset()}
              className={`focus-ring press flex shrink-0 cursor-pointer items-center gap-1.5 rounded-[9px] px-3 py-2 text-[12px] font-semibold disabled:cursor-default disabled:opacity-50 ${
                resetConfirming
                  ? "bg-danger text-white hover:opacity-90"
                  : "bg-danger-soft text-danger hover:bg-danger hover:text-white"
              }`}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              {resetConfirming ? "Reset now" : "Reset"}
            </button>
          </div>
          {resetConfirming ? (
            <div className="mt-2 flex items-center justify-between gap-2 rounded-[8px] bg-danger-soft px-2.5 py-2 text-[11px] text-danger">
              <span>This cannot be undone.</span>
              <button
                type="button"
                onClick={() => setResetConfirming(false)}
                className="focus-ring press shrink-0 rounded-[6px] px-2 py-1 font-semibold hover:bg-surface-hover"
              >
                Cancel
              </button>
            </div>
          ) : null}
        </div>

        {persistenceError ? (
          <div
            role="alert"
            className="mx-5 mb-3 flex items-center justify-between gap-3 rounded-[10px] bg-danger-soft px-3 py-2.5 text-danger"
          >
            <span className="min-w-0 text-[11.5px] leading-relaxed">{persistenceError}</span>
            <button
              type="button"
              disabled={retrySaveBusy}
              aria-busy={retrySaveBusy}
              onClick={() => void retrySave()}
              className="focus-ring press min-h-11 shrink-0 cursor-pointer rounded-[8px] bg-surface px-3 text-[11.5px] font-semibold hover:bg-surface-hover disabled:cursor-default disabled:opacity-50"
            >
              {retrySaveBusy ? "Saving..." : "Retry save"}
            </button>
          </div>
        ) : null}

        <div className="scroll-thin flex-1 overflow-y-auto px-5 pb-6">
          <SectionTitle>Appearance</SectionTitle>
          <SettingsRow title="Appearance" detail="Follows Windows light/dark mode">
            <Segmented<ThemeMode>
              label="Appearance"
              value={settings.theme}
              onChange={(theme) => updateSettings({ theme })}
              options={THEME_OPTIONS}
            />
          </SettingsRow>
          <SettingsRow title="Accent color" detail="Used for highlights and focus">
            <div className="flex gap-2">
              {ACCENTS.map((a) => {
                const active = settings.accent === a.id;
                return (
                  <button
                    key={a.id}
                    type="button"
                    aria-label={`Accent ${a.name}`}
                    aria-pressed={active}
                    onClick={() => updateSettings({ accent: a.id })}
                    className={`focus-ring relative h-6 w-6 cursor-pointer rounded-full transition-transform duration-150 after:absolute after:-inset-1 after:rounded-full after:content-[''] ${
                      active ? "scale-110" : "hover:scale-105"
                    }`}
                    style={{
                      background: `var(--accent-${a.id})`,
                      boxShadow: active
                        ? "0 0 0 2px var(--t-bg), 0 0 0 4px var(--accent)"
                        : "inset 0 1px 0 rgb(255 255 255 / 0.2), 0 1px 3px rgb(0 0 0 / 0.4)",
                    }}
                  />
                );
              })}
            </div>
          </SettingsRow>
          <SettingsRow title="Window width">
            <Segmented<WindowWidth>
              label="Window width"
              value={settings.width}
              onChange={(width) => updateSettings({ width })}
              options={[
                { value: 560, label: "S" },
                { value: 640, label: "M" },
                { value: 720, label: "L" },
              ]}
            />
          </SettingsRow>
          <SettingsRow title="View zoom" detail="Ctrl + Up/Down or Ctrl + wheel">
            <ViewZoomControl />
          </SettingsRow>

          <SectionTitle>Taskbar</SectionTitle>
          <SettingsRow title="Alignment">
            <TaskbarAlignmentPicker />
          </SettingsRow>
          <SettingsRow title="Scroll volume" detail="Scroll mouse wheel over taskbar apps to adjust volume">
            <Toggle
              checked={settings.taskbarScrollVolume ?? true}
              onChange={(v) => updateSettings({ taskbarScrollVolume: v })}
              label="Scroll taskbar to adjust volume"
            />
          </SettingsRow>
          <TaskbarCustomization />

          <SectionTitle>Behavior</SectionTitle>
          <SettingsRow title="Global shortcut" detail="Open Prism from anywhere">
            <KeybindPicker />
          </SettingsRow>
          <SettingsRow title="Keep on top" detail="Prism floats above other windows">
            <Toggle
              checked={settings.alwaysOnTop}
              onChange={(v) => updateSettings({ alwaysOnTop: v })}
              label="Keep window on top"
            />
          </SettingsRow>
          <SettingsRow title="Quick Access" detail="Choose up to 6 pinned folders">
            <QuickAccessPicker />
          </SettingsRow>
          <SettingsRow title="App collections" detail="Group apps like Creative, Developer, or Games">
            <AppGroupsPicker />
          </SettingsRow>
          <SettingsRow title="Recent items" detail="Items are kept in order of use">
            <button
              type="button"
              onClick={() => clearHistory()}
              className="focus-ring press flex cursor-pointer items-center gap-1.5 rounded-[10px] bg-surface px-3 py-1.5 text-[12px] font-medium text-fg-secondary hover:bg-surface-hover hover:text-fg"
            >
              <History className="h-3.5 w-3.5" />
              Clear
            </button>
          </SettingsRow>

          <SectionTitle>About</SectionTitle>
          <div className="flex items-center justify-between py-3">
            <div>
              <div className="text-[13px] font-medium text-fg">Prism</div>
              <div className="mt-0.5 text-[11.5px] text-fg-tertiary">
                {version ? `Version ${version}` : "Version"}
              </div>
            </div>
            <button
              type="button"
              onClick={() => void quit()}
              className="focus-ring press flex min-h-11 cursor-pointer items-center gap-1.5 rounded-[10px] bg-danger-soft px-3 text-[12px] font-medium text-danger hover:opacity-90"
            >
              <LogOut className="h-3.5 w-3.5" />
              Quit
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <div className="border-t border-line pb-1 pt-4 text-[11px] font-semibold text-fg-quiet uppercase first:border-t-0 first:pt-0">
      {children}
    </div>
  );
}
