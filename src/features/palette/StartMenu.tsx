// biome-ignore-all lint/a11y/useSemanticElements: the menu rows use button layout so rows can carry icons and context menus.
import { Folder } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { RowIcon, SectionLabel } from "../../components/ui";
import type { PaletteItem } from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";
import { buildMenuModel, FREQUENT_LIMIT } from "./menuModel";
import { type ContextMenuPosition, clampContextMenuPosition, ResultContextMenu } from "./ResultContextMenu";
import { appPaletteItem } from "./sections";

const PAGE_SIZE = 8;

/** Actions Palette's search box can drive in the menu view via this handle. */
export interface StartMenuHandle {
  move: (delta: number) => void;
  jump: (edge: "first" | "last") => void;
  pageMove: (delta: -1 | 1) => void;
  runSelected: () => void;
  openSelectedMenu: () => void;
}

interface ResultMenuState {
  item: PaletteItem;
  position: ContextMenuPosition;
}

/** Any action that justifies opening the result context menu. */
function hasResultActions(item: PaletteItem | undefined): item is PaletteItem {
  return Boolean(
    item && (item.openLocation || item.runAsAdmin || item.toggleTaskbarPin || item.showProperties),
  );
}

/** Context-menu viewport height from the actions the item actually has. */
function resultMenuHeight(item: PaletteItem): number {
  return (
    ((item.openLocation ? 1 : 0) +
      (item.runAsAdmin ? 1 : 0) +
      (item.appId ? 1 : 0) +
      (item.toggleTaskbarPin ? 1 : 0) +
      (item.showProperties ? 1 : 0)) *
      36 +
    8
  );
}

/**
 * The start-menu view: pinned tiles, frequent apps and quick access on the
 * left, the All apps folder tree on the right. Rendered only while the query
 * is empty; any typed character falls back to the normal results palette.
 *
 * `narrow` adapts the layout to the 560px window width: a slimmer left
 * column and a two-wide pinned grid instead of three.
 */
export const StartMenu = forwardRef<StartMenuHandle, { narrow?: boolean }>(function StartMenu(
  { narrow },
  ref,
) {
  const palette = usePalette();
  const app = useApp();
  const { settings, updateSettings, showToast } = app;

  const [selected, setSelected] = useState(0);
  const [resultMenu, setResultMenu] = useState<ResultMenuState | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const model = useMemo(
    () =>
      buildMenuModel({
        apps: palette.apps,
        pinnedApps: [],
        pinnedAppIds: settings.pinnedApps,
        history: app.history,
        appIcons: palette.appIcons,
      }),
    [palette.apps, settings.pinnedApps, app.history, palette.appIcons],
  );

  // Pinned tiles resolve in the user's pinned order on every settings change.
  const pinnedItems = useMemo(() => {
    const appsById = new Map(palette.apps.map((entry) => [entry.appId, entry]));
    return settings.pinnedApps.flatMap((appId) => {
      const entry = appsById.get(appId);
      return entry ? [appPaletteItem(entry, palette.appIcons)] : [];
    });
  }, [palette.apps, palette.appIcons, settings.pinnedApps]);

  const programs = model.programFlat;
  const selectedRef = useRef(0);
  selectedRef.current = selected;

  useEffect(() => {
    setSelected((previous) => Math.min(previous, Math.max(0, programs.length - 1)));
  }, [programs.length]);

  const clamp = useCallback(
    (index: number) => Math.min(Math.max(index, 0), Math.max(0, programs.length - 1)),
    [programs.length],
  );
  const move = useCallback((delta: number) => setSelected((previous) => clamp(previous + delta)), [clamp]);
  const jump = useCallback(
    (edge: "first" | "last") => setSelected(edge === "first" ? 0 : Math.max(0, programs.length - 1)),
    [programs.length],
  );
  const runItem = useCallback(
    (item: PaletteItem) => {
      void palette.runItem(item);
    },
    [palette],
  );
  const runSelected = useCallback(() => {
    const item = programs[selectedRef.current];
    if (item) runItem(item);
  }, [programs, runItem]);
  const pageMove = useCallback((delta: -1 | 1) => move(delta * PAGE_SIZE), [move]);

  const openResultMenu = useCallback((item: PaletteItem, x: number, y: number) => {
    if (!hasResultActions(item)) return;
    setResultMenu({ item, position: clampContextMenuPosition(x, y, resultMenuHeight(item)) });
  }, []);

  const openSelectedMenu = useCallback(() => {
    const item = programs[selectedRef.current];
    if (!item) return;
    const row = document.querySelector<HTMLElement>(`[data-menu-item='${CSS.escape(item.id)}']`);
    const bounds = row?.getBoundingClientRect();
    openResultMenu(
      item,
      bounds ? bounds.right - 12 : window.innerWidth / 2,
      bounds ? bounds.top + 12 : window.innerHeight / 2,
    );
  }, [programs, openResultMenu]);

  useImperativeHandle(ref, () => ({ move, jump, pageMove, runSelected, openSelectedMenu }), [
    move,
    jump,
    pageMove,
    runSelected,
    openSelectedMenu,
  ]);

  // Keep the selection in view while scrolling.
  useEffect(() => {
    if (selected < 0) return;
    const el = listRef.current?.querySelector("[data-menu-selected='true']");
    el?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const closeResultMenu = useCallback((restoreFocus: boolean) => {
    setResultMenu(null);
    if (restoreFocus) {
      requestAnimationFrame(() => document.querySelector<HTMLInputElement>("[data-prism-search]")?.focus());
    }
  }, []);

  const togglePin = useCallback(
    (item: PaletteItem) => {
      const appId = item.appId;
      if (!appId) return;
      const pinned = settings.pinnedApps.includes(appId);
      if (pinned) {
        updateSettings({ pinnedApps: settings.pinnedApps.filter((candidate) => candidate !== appId) });
        showToast("Unpinned", item.title);
        return;
      }
      updateSettings({ pinnedApps: [...settings.pinnedApps, appId] });
      showToast("Pinned", item.title);
    },
    [settings.pinnedApps, showToast, updateSettings],
  );

  const appsLoading = !palette.appsLoaded;
  const frequent = model.frequent.slice(0, FREQUENT_LIMIT);

  return (
    <div className="flex min-h-0 flex-1 gap-0 px-2.5 pb-2" data-testid="start-menu">
      {/* ----- left column: pinned, frequent, quick access ----- */}
      <div
        className={`scroll-thin flex min-w-0 flex-col overflow-y-auto pe-2 ${narrow ? "w-[42%]" : "w-[46%]"}`}
      >
        <div className="px-2 pb-0.5">
          <SectionLabel>Pinned</SectionLabel>
        </div>
        {pinnedItems.length === 0 ? (
          <p className="px-2 pb-1 text-[11.5px] text-fg-tertiary">
            Right-click an app and choose pin to keep it here.
          </p>
        ) : (
          <div className={`grid gap-1 ${narrow ? "grid-cols-2" : "grid-cols-3"}`}>
            {pinnedItems.map((item) => (
              <button
                key={item.id}
                type="button"
                className="focus-ring press flex cursor-pointer flex-col items-center gap-1.5 rounded-[10px] px-1.5 py-2.5 hover:bg-surface-hover"
                onClick={() => runItem(item)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  openResultMenu(item, event.clientX, event.clientY);
                }}
              >
                <RowIcon icon={item.icon} size={40} />
                <span className="w-full truncate text-center text-[11.5px] leading-tight text-fg-secondary">
                  {item.title}
                </span>
              </button>
            ))}
          </div>
        )}

        {frequent.length > 0 ? (
          <>
            <div className="px-2 pb-0.5 pt-3">
              <SectionLabel>Frequent</SectionLabel>
            </div>
            <div className="flex list-none flex-col gap-[2px] p-0">
              {frequent.map((item) => (
                <MenuRow
                  key={item.id}
                  item={item}
                  selected={false}
                  onRun={runItem}
                  onOpenMenu={openResultMenu}
                  compact
                />
              ))}
            </div>
          </>
        ) : null}

        {palette.quickItems.length > 0 ? (
          <>
            <div className="px-2 pb-0.5 pt-3">
              <SectionLabel>Quick Access</SectionLabel>
            </div>
            <div className="flex list-none flex-col gap-[2px] p-0">
              {palette.quickItems.map((item) => (
                <MenuRow
                  key={item.id}
                  item={item}
                  selected={false}
                  onRun={runItem}
                  onOpenMenu={openResultMenu}
                  compact
                />
              ))}
            </div>
          </>
        ) : null}
      </div>

      {/* ----- right column: all apps folder tree ----- */}
      <div className="flex min-w-0 flex-1 flex-col border-s border-line ps-2.5">
        <div className="flex items-center justify-between px-2 pb-0.5">
          <SectionLabel>All apps</SectionLabel>
          <span className="text-[10.5px] text-fg-quiet">{programs.length}</span>
        </div>
        <div
          ref={listRef}
          role="listbox"
          aria-label="All apps"
          className="scroll-thin min-h-0 flex-1 overflow-y-auto pe-1"
        >
          {appsLoading ? (
            <p className="px-2 py-3 text-[12px] text-fg-tertiary">Loading appsâ€¦</p>
          ) : programs.length === 0 ? (
            <p className="px-2 py-3 text-[12px] text-fg-tertiary">No installed apps found.</p>
          ) : (
            model.programGroups.map((group) => (
              <div key={group.id} role="presentation" className="menu-group">
                <div className="flex items-center gap-1.5 px-2 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wide text-fg-quiet">
                  <Folder className="h-3 w-3" aria-hidden="true" />
                  <span className="truncate">{group.label}</span>
                </div>
                <div className="flex list-none flex-col gap-[2px] p-0">
                  {group.items.map((item) => (
                    <MenuRow
                      key={item.id}
                      item={item}
                      selected={programs[selected] === item}
                      onRun={runItem}
                      onOpenMenu={openResultMenu}
                    />
                  ))}
                </div>
              </div>
            ))
          )}
        </div>
      </div>

      {resultMenu ? (
        <ResultContextMenu
          item={resultMenu.item}
          position={resultMenu.position}
          appPinned={resultMenu.item.appId ? settings.pinnedApps.includes(resultMenu.item.appId) : false}
          onToggleAppPin={
            resultMenu.item.appId
              ? () => {
                  const { item } = resultMenu;
                  setResultMenu(null);
                  togglePin(item);
                }
              : undefined
          }
          onOpenLocation={() => {
            const { item } = resultMenu;
            setResultMenu(null);
            item.openLocation?.();
          }}
          onRunAsAdmin={() => {
            const { item } = resultMenu;
            setResultMenu(null);
            void palette.runItemAsAdmin(item);
          }}
          onToggleTaskbarPin={(wasPinned) => {
            const { item } = resultMenu;
            setResultMenu(null);
            void (async () => {
              try {
                await item.toggleTaskbarPin?.();
                showToast(wasPinned ? "Unpinned from taskbar" : "Pinned to taskbar", item.title);
              } catch (error) {
                showToast("Could not update taskbar pin", String(error));
              }
            })();
          }}
          onShowProperties={() => {
            const { item } = resultMenu;
            setResultMenu(null);
            void (async () => {
              try {
                await item.showProperties?.();
              } catch (error) {
                showToast("Could not open properties", String(error));
              }
            })();
          }}
          onClose={closeResultMenu}
        />
      ) : null}
    </div>
  );
});

function MenuRow({
  item,
  selected,
  onRun,
  onOpenMenu,
  compact,
}: {
  item: PaletteItem;
  selected: boolean;
  onRun: (item: PaletteItem) => void;
  onOpenMenu: (item: PaletteItem, x: number, y: number) => void;
  compact?: boolean;
}) {
  return (
    <button
      type="button"
      role="option"
      aria-selected={selected}
      data-menu-item={item.id}
      data-menu-selected={selected || undefined}
      onClick={() => onRun(item)}
      onContextMenu={(event) => {
        event.preventDefault();
        onOpenMenu(item, event.clientX, event.clientY);
      }}
    >
      <RowIcon icon={item.icon} size={compact ? 26 : 30} />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] leading-tight text-fg">{item.title}</span>
        <span className="mt-[1px] block truncate text-[10.5px] leading-tight text-fg-tertiary">
          {item.subtitle ?? "Application"}
        </span>
      </span>
    </button>
  );
}
