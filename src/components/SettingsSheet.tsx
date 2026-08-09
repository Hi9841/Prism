import { History, Keyboard, LogOut, Minus, Plus, RotateCcw, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { quitApp, setShortcut } from "../lib/bridge";
import type { AccentId, ThemeMode, WindowEffect, WindowWidth } from "../lib/types";
import { DEFAULT_SETTINGS, stepViewZoom, VIEW_ZOOM_LEVELS } from "../lib/types";
import { SHORTCUT_OPTIONS, THEME_OPTIONS, useApp } from "../state/app";
import { Segmented, Toggle } from "./ui";

const ACCENTS: { id: AccentId; name: string }[] = [
  { id: "iris", name: "Iris" },
  { id: "azure", name: "Azure" },
  { id: "mint", name: "Mint" },
  { id: "amber", name: "Amber" },
  { id: "rose", name: "Rose" },
];

function Row({ title, detail, children }: { title: string; detail?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-3">
      <div className="min-w-0 flex-1 basis-40">
        <div className="text-[13px] font-medium text-fg">{title}</div>
        {detail ? <div className="mt-0.5 text-[11.5px] text-fg-tertiary">{detail}</div> : null}
      </div>
      <div className="min-w-0 max-w-full shrink-0">{children}</div>
    </div>
  );
}

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
              className={`focus-ring cursor-pointer rounded-[9px] px-2 py-[4px] text-[11px] font-medium transition-colors duration-150 ${
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
        className="focus-ring grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-tertiary transition-colors hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-35"
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
        className="focus-ring grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-tertiary transition-colors hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-35"
      >
        <Plus className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        aria-label="Reset zoom"
        title="Reset zoom"
        disabled={settings.viewZoom === DEFAULT_SETTINGS.viewZoom}
        onClick={() => updateSettings({ viewZoom: DEFAULT_SETTINGS.viewZoom })}
        className="focus-ring grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] text-fg-tertiary transition-colors hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-35"
      >
        <RotateCcw className="h-3.5 w-3.5" />
      </button>
    </fieldset>
  );
}

export function SettingsSheet() {
  const { settings, updateSettings, openSettings, setOpenSettings, clearHistory } = useApp();
  const panelRef = useRef<HTMLDivElement>(null);
  const [titleId] = useState(() => `prism-settings-title-${Math.random().toString(36).slice(2, 8)}`);

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
    <div className="settings-backdrop absolute inset-0 z-40 flex">
      <button
        type="button"
        aria-label="Close settings"
        className="absolute inset-0 cursor-default rounded-[26px_26px_8px_8px] bg-backdrop backdrop-blur-[2px]"
        onClick={() => setOpenSettings(false)}
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onPanelKeyDown}
        className="settings-panel relative ml-auto flex w-[min(340px,88%)] flex-col overflow-hidden rounded-tr-[26px] rounded-br-[8px] border-l border-line bg-bg-raised backdrop-blur-2xl"
        style={{ boxShadow: "-24px 0 64px rgb(0 0 0 / 0.35)" }}
      >
        <div className="flex items-center justify-between px-5 pb-2 pt-5">
          <h2 id={titleId} className="text-[15px] font-semibold text-fg">
            Settings
          </h2>
          <button
            type="button"
            aria-label="Close settings"
            onClick={() => setOpenSettings(false)}
            className="focus-ring grid h-7 w-7 cursor-pointer place-items-center rounded-lg text-fg-tertiary transition-colors hover:bg-surface-hover hover:text-fg-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="scroll-thin flex-1 overflow-y-auto px-5 pb-6">
          <SectionTitle>Appearance</SectionTitle>
          <Row title="Appearance" detail="Follows Windows light/dark mode">
            <Segmented<ThemeMode>
              label="Appearance"
              value={settings.theme}
              onChange={(theme) => updateSettings({ theme })}
              options={THEME_OPTIONS}
            />
          </Row>
          <Row title="Accent color" detail="Used for highlights and focus">
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
                    className={`focus-ring h-6 w-6 cursor-pointer rounded-full transition-transform duration-150 ${
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
          </Row>
          <Row title="Window width">
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
          </Row>
          <Row title="View zoom" detail="Ctrl + Up/Down or Ctrl + wheel">
            <ViewZoomControl />
          </Row>
          <Row title="Window material" detail="Solid avoids DWM blur issues">
            <Segmented<WindowEffect>
              label="Window material"
              value={settings.effect}
              onChange={(effect) => updateSettings({ effect })}
              options={[
                { value: "acrylic", label: "Acrylic" },
                { value: "mica", label: "Mica" },
                { value: "solid", label: "Solid" },
              ]}
            />
          </Row>

          <SectionTitle>Behavior</SectionTitle>
          <Row title="Global shortcut" detail="Open Prism from anywhere">
            <KeybindPicker />
          </Row>
          <Row title="Keep on top" detail="Prism floats above other windows">
            <Toggle
              checked={settings.alwaysOnTop}
              onChange={(v) => updateSettings({ alwaysOnTop: v })}
              label="Keep window on top"
            />
          </Row>
          <Row title="Recent items" detail="Items are kept in order of use">
            <button
              type="button"
              onClick={() => clearHistory()}
              className="focus-ring flex cursor-pointer items-center gap-1.5 rounded-[10px] bg-surface px-3 py-1.5 text-[12px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover hover:text-fg"
            >
              <History className="h-3.5 w-3.5" />
              Clear
            </button>
          </Row>

          <SectionTitle>About</SectionTitle>
          <div className="flex items-center justify-between py-3">
            <div>
              <div className="text-[13px] font-medium text-fg">Prism</div>
              <div className="mt-0.5 text-[11.5px] text-fg-tertiary">Version 0.3.0</div>
            </div>
            <button
              type="button"
              onClick={() => quitApp()}
              className="focus-ring flex cursor-pointer items-center gap-1.5 rounded-[10px] bg-danger-soft px-3 py-1.5 text-[12px] font-medium text-danger transition-colors hover:opacity-90"
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
