import { GripVertical, Pin, PinOff, RefreshCw, Search, Settings2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { PowerMenu } from "../../components/PowerMenu";
import { IconButton, Kbd, RowIcon, SectionLabel } from "../../components/ui";
import type { PaletteItem } from "../../lib/types";
import { PINNED_APP_LIMIT, reorderPinnedApps } from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";
import { UpdateControl } from "../updater/UpdateControl";

interface PinDragState {
  item: PaletteItem;
  pointerId: number;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  targetAppId: string | null;
}

export function Palette() {
  const palette = usePalette();
  const app = useApp();
  const { settings, setOpenSettings } = app;

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const pinDragRef = useRef<PinDragState | null>(null);
  const pendingPinDragRef = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const pinDragFrameRef = useRef<number | null>(null);
  const [pinDrag, setPinDrag] = useState<PinDragState | null>(null);

  const togglePin = useCallback(
    (item: PaletteItem) => {
      const appId = item.appId;
      if (!appId) return;
      const pinned = settings.pinnedApps.includes(appId);
      if (pinned) {
        app.updateSettings({
          pinnedApps: settings.pinnedApps.filter((candidate) => candidate !== appId),
        });
        app.showToast("Unpinned", item.title);
        requestAnimationFrame(() => inputRef.current?.focus());
        return;
      }
      if (settings.pinnedApps.length >= PINNED_APP_LIMIT) {
        app.showToast("Pin limit reached", `Unpin an app before adding ${item.title}`);
        return;
      }
      app.updateSettings({
        pinnedApps: [...settings.pinnedApps, appId],
      });
      app.showToast("Pinned", item.title);
      requestAnimationFrame(() => inputRef.current?.focus());
    },
    [app, settings.pinnedApps],
  );

  const reorderPin = useCallback(
    (sourceAppId: string, targetAppId: string) => {
      const pinnedApps = reorderPinnedApps(settings.pinnedApps, sourceAppId, targetAppId);
      if (pinnedApps.every((appId, index) => appId === settings.pinnedApps[index])) return;
      app.updateSettings({ pinnedApps });
    },
    [app, settings.pinnedApps],
  );

  const movePin = useCallback(
    (appId: string, direction: -1 | 1) => {
      const index = settings.pinnedApps.indexOf(appId);
      const targetAppId = settings.pinnedApps[index + direction];
      if (index < 0 || !targetAppId) return;
      reorderPin(appId, targetAppId);
    },
    [reorderPin, settings.pinnedApps],
  );

  const updatePinDragState = useCallback((next: PinDragState | null) => {
    pinDragRef.current = next;
    setPinDrag(next);
  }, []);

  const startPinDrag = useCallback(
    (item: PaletteItem, pointerId: number, x: number, y: number) => {
      updatePinDragState({
        item,
        pointerId,
        startX: x,
        startY: y,
        x,
        y,
        active: false,
        targetAppId: null,
      });
    },
    [updatePinDragState],
  );

  const applyPinDrag = useCallback(
    (pointerId: number, x: number, y: number) => {
      const current = pinDragRef.current;
      if (!current || current.pointerId !== pointerId) return;

      const active = current.active || Math.hypot(x - current.startX, y - current.startY) >= 4;
      const hoveredRow = active
        ? document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-pinned-app-id]")
        : null;
      const hoveredAppId = hoveredRow?.dataset.pinnedAppId ?? null;
      const targetAppId = hoveredAppId === current.item.appId ? null : hoveredAppId;
      updatePinDragState({ ...current, x, y, active, targetAppId });
    },
    [updatePinDragState],
  );

  const updatePinDrag = useCallback(
    (pointerId: number, x: number, y: number) => {
      pendingPinDragRef.current = { pointerId, x, y };
      if (pinDragFrameRef.current !== null) return;
      pinDragFrameRef.current = requestAnimationFrame(() => {
        pinDragFrameRef.current = null;
        const pending = pendingPinDragRef.current;
        pendingPinDragRef.current = null;
        if (pending) applyPinDrag(pending.pointerId, pending.x, pending.y);
      });
    },
    [applyPinDrag],
  );

  const finishPinDrag = useCallback(
    (pointerId: number) => {
      if (pinDragFrameRef.current !== null) {
        cancelAnimationFrame(pinDragFrameRef.current);
        pinDragFrameRef.current = null;
      }
      const pending = pendingPinDragRef.current;
      pendingPinDragRef.current = null;
      if (pending) applyPinDrag(pending.pointerId, pending.x, pending.y);
      const current = pinDragRef.current;
      if (!current || current.pointerId !== pointerId) return;
      if (current.active && current.item.appId && current.targetAppId) {
        reorderPin(current.item.appId, current.targetAppId);
      }
      updatePinDragState(null);
    },
    [applyPinDrag, reorderPin, updatePinDragState],
  );

  const cancelPinDrag = useCallback(
    (pointerId: number) => {
      pendingPinDragRef.current = null;
      if (pinDragFrameRef.current !== null) {
        cancelAnimationFrame(pinDragFrameRef.current);
        pinDragFrameRef.current = null;
      }
      if (pinDragRef.current?.pointerId === pointerId) updatePinDragState(null);
    },
    [updatePinDragState],
  );

  useEffect(() => {
    if (!pinDrag) return;
    const cancelOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") updatePinDragState(null);
    };
    window.addEventListener("keydown", cancelOnEscape);
    return () => window.removeEventListener("keydown", cancelOnEscape);
  }, [pinDrag, updatePinDragState]);

  useEffect(
    () => () => {
      if (pinDragFrameRef.current !== null) cancelAnimationFrame(pinDragFrameRef.current);
    },
    [],
  );

  /* Keyboard-first navigation. */
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          palette.move(1);
          break;
        case "ArrowUp":
          e.preventDefault();
          palette.move(-1);
          break;
        case "Enter":
          e.preventDefault();
          palette.runSelected();
          break;
        case "Escape":
          e.preventDefault();
          if (app.openSettings) {
            setOpenSettings(false);
          } else if (palette.query) {
            palette.setQuery("");
          } else {
            document.dispatchEvent(new CustomEvent("prism:close"));
          }
          break;
        case "k":
          if (e.ctrlKey || e.metaKey) {
            e.preventDefault();
            document.dispatchEvent(new CustomEvent("prism:close"));
          }
          break;
        case "Backspace":
          if (e.ctrlKey) {
            e.preventDefault();
            palette.setQuery("");
          }
          break;
      }
    },
    [palette, app.openSettings, setOpenSettings],
  );

  // The component mounts fresh each time the window is shown - grab focus
  // for keyboard-first interaction, with the caret at the end of the query.
  useEffect(() => {
    const input = inputRef.current;
    if (input) {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    }
  }, []);

  // Keep the selection in view while scrolling.
  useEffect(() => {
    if (palette.selected < 0) return;
    const el = listRef.current?.querySelector("[data-selected='true']");
    el?.scrollIntoView({ block: "nearest" });
  }, [palette.selected]);

  return (
    <div className="shell focus-ring" style={{ height: "100%" }}>
      {/* ------- header ------- */}
      <div className="px-5 pb-1 pt-5">
        <div className="search-field flex items-center gap-3 px-4 py-3">
          <Search className="h-[18px] w-[18px] shrink-0 text-fg-tertiary" strokeWidth={2} />
          <input
            ref={inputRef}
            data-prism-search
            type="text"
            aria-controls="prism-results"
            aria-label="Search files, folders, or apps, or type a calculation"
            value={palette.query}
            onChange={(e) => palette.setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Search files, folders, and apps…"
            spellCheck={false}
            autoComplete="off"
            className="min-w-0 flex-1 bg-transparent text-[15px] font-medium text-fg outline-none placeholder:text-fg-quiet"
          />
          <Kbd>{settings.shortcut}</Kbd>
          <IconButton
            label="Settings"
            data-settings-trigger
            active={app.openSettings}
            onClick={() => setOpenSettings(!app.openSettings)}
          >
            <Settings2 className="h-4 w-4" strokeWidth={2} />
          </IconButton>
        </div>
      </div>

      {/* ------- results ------- */}
      <div
        ref={listRef}
        id="prism-results"
        aria-busy={!palette.appsLoaded || palette.filesBusy}
        className="scroll-thin min-h-0 flex-1 overflow-y-auto px-2.5 pb-2"
      >
        {palette.flatItems.length === 0 ? (
          <EmptyState
            query={palette.query}
            loading={palette.query ? palette.filesBusy : !palette.appsLoaded}
            error={palette.query ? palette.filesError : palette.appsError}
            pathBrowsing={palette.pathBrowsing}
            onRetry={palette.refreshApps}
          />
        ) : (
          (() => {
            let flat = 0;
            return palette.sections.map((section) => (
              <div key={section.id}>
                <SectionLabel>{section.label}</SectionLabel>
                <ul aria-label={section.label} className="m-0 flex list-none flex-col gap-[2px] p-0">
                  {section.items.map((item) => {
                    const index = flat++;
                    const reorderable = section.id === "pinned";
                    return (
                      <ResultRow
                        key={item.id}
                        item={item}
                        index={index}
                        selected={palette.selected === index}
                        pinned={item.appId ? settings.pinnedApps.includes(item.appId) : false}
                        reorderable={reorderable}
                        draggedPin={pinDrag?.active ? (pinDrag.item.appId ?? null) : null}
                        dropTargetPin={pinDrag?.active ? pinDrag.targetAppId : null}
                        onSelect={palette.select}
                        onRun={palette.runItem}
                        onTogglePin={togglePin}
                        onMovePin={movePin}
                        onStartPinDrag={startPinDrag}
                        onUpdatePinDrag={updatePinDrag}
                        onFinishPinDrag={finishPinDrag}
                        onCancelPinDrag={cancelPinDrag}
                      />
                    );
                  })}
                </ul>
              </div>
            ));
          })()
        )}
      </div>

      {pinDrag?.active ? <PinDragPreview drag={pinDrag} /> : null}

      {/* ------- footer ------- */}
      <div className="footer-bar flex min-h-12 items-center justify-between px-5 py-2.5">
        <div className="flex items-center gap-1.5 text-[11px] text-fg-quiet">
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd>
          <span className="px-1">navigate</span>
          <span className="text-divider">·</span>
          <Kbd>↵</Kbd>
          <span className="px-1">open</span>
          <span className="text-divider">·</span>
          <Kbd>esc</Kbd>
          <span className="px-1">dismiss</span>
        </div>
        <div className="flex items-center gap-1.5">
          <UpdateControl />
          {!palette.appsLoaded && palette.query === "" && (
            <span className="flex items-center gap-1.5 text-[11px] text-fg-quiet">
              <RefreshCw className="h-3 w-3 animate-spin" />
              indexing apps
            </span>
          )}
          {palette.fileIndexing && palette.appsLoaded && (
            <span className="flex items-center gap-1.5 text-[11px] text-fg-quiet">
              <RefreshCw className="h-3 w-3 animate-spin" />
              indexing files
            </span>
          )}
          {palette.appsError && palette.query === "" && (
            <span className="text-[11px] text-danger">apps failed to load</span>
          )}
          <IconButton
            label="Refresh applications"
            onClick={palette.refreshApps}
            disabled={!palette.appsLoaded}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </IconButton>
          <PowerMenu />
        </div>
      </div>
    </div>
  );
}

function PinDragPreview({ drag }: { drag: PinDragState }) {
  const previewWidth = 232;
  const previewHeight = 54;
  const x = Math.min(Math.max(8, drag.x - previewWidth - 14), window.innerWidth - previewWidth - 8);
  const y = Math.min(Math.max(8, drag.y - previewHeight / 2), window.innerHeight - previewHeight - 8);

  return (
    <div
      aria-hidden="true"
      className="pointer-events-none fixed top-0 left-0 z-50 will-change-transform"
      style={{ transform: `translate3d(${x}px, ${y}px, 0)` }}
    >
      <div className="pin-drag-preview">
        <RowIcon icon={drag.item.icon} />
        <div className="min-w-0">
          <div className="truncate text-[13.5px] leading-tight font-semibold text-fg">{drag.item.title}</div>
          <div className="mt-[3px] truncate text-[11.5px] leading-tight text-fg-tertiary">
            {drag.item.subtitle ?? "Application"}
          </div>
        </div>
        <GripVertical className="h-4 w-4 shrink-0 text-accent" />
      </div>
    </div>
  );
}

function EmptyState({
  query,
  loading,
  error,
  pathBrowsing,
  onRetry,
}: {
  query: string;
  loading: boolean;
  error: boolean;
  pathBrowsing: boolean;
  onRetry: () => void;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2.5 text-center">
      <div className="grid h-14 w-14 place-items-center rounded-[20px] bg-surface">
        <Search className="h-6 w-6 text-fg-quiet" strokeWidth={1.75} />
      </div>
      {error ? (
        <>
          <div className="text-[13.5px] font-medium text-fg-secondary">
            {query ? "File search is unavailable" : "Apps couldn't be loaded"}
          </div>
          <div className="max-w-[260px] text-[12px] leading-relaxed text-fg-tertiary">
            {query
              ? "Prism couldn't read the local file index. Installed apps are still available."
              : "The app index failed to scan. Retry now, or keep searching local files."}
          </div>
          {!query && (
            <button
              type="button"
              onClick={onRetry}
              className="focus-ring mt-1 flex cursor-pointer items-center gap-1.5 rounded-[10px] bg-surface px-3 py-1.5 text-[12px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover hover:text-fg"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              Retry
            </button>
          )}
        </>
      ) : (
        <>
          <div className="text-[13.5px] font-medium text-fg-secondary">
            {loading
              ? query
                ? "Searching local files…"
                : "Preparing Prism…"
              : pathBrowsing
                ? "No matching items"
                : "Start typing to search"}
          </div>
          <div className="max-w-[260px] text-[12px] leading-relaxed text-fg-tertiary">
            {loading
              ? query
                ? "The first file index finishes in the background"
                : "Loading quick access and installed applications"
              : pathBrowsing
                ? "This folder is empty, or no item matches the partial path"
                : "Find local files, folders, apps, or calculate 12 × 8"}
          </div>
        </>
      )}
    </div>
  );
}

function ResultRow({
  item,
  index,
  selected,
  pinned,
  reorderable,
  draggedPin,
  dropTargetPin,
  onSelect,
  onRun,
  onTogglePin,
  onMovePin,
  onStartPinDrag,
  onUpdatePinDrag,
  onFinishPinDrag,
  onCancelPinDrag,
}: {
  item: PaletteItem;
  index: number;
  selected: boolean;
  pinned: boolean;
  reorderable: boolean;
  draggedPin: string | null;
  dropTargetPin: string | null;
  onSelect: (i: number) => void;
  onRun: (item: PaletteItem) => void;
  onTogglePin: (item: PaletteItem) => void;
  onMovePin: (appId: string, direction: -1 | 1) => void;
  onStartPinDrag: (item: PaletteItem, pointerId: number, x: number, y: number) => void;
  onUpdatePinDrag: (pointerId: number, x: number, y: number) => void;
  onFinishPinDrag: (pointerId: number) => void;
  onCancelPinDrag: (pointerId: number) => void;
}) {
  const appId = item.appId;
  const canReorder = reorderable && Boolean(appId);

  return (
    <li
      id={`prism-opt-${index}`}
      data-selected={selected}
      data-drop-target={canReorder && dropTargetPin === appId}
      data-dragging={canReorder && draggedPin === appId}
      data-pinned-app-id={canReorder ? appId : undefined}
      onMouseEnter={() => onSelect(index)}
      className={`group row w-full text-left transition-[background-color,box-shadow] duration-100 ${
        selected ? "bg-surface-active" : "hover:bg-surface-hover"
      }`}
    >
      <button
        type="button"
        tabIndex={-1}
        aria-label={`Open ${item.title}`}
        onClick={() => onRun(item)}
        className="focus-ring absolute inset-0 z-0 cursor-pointer rounded-[10px]"
      />
      <div className="pointer-events-none relative z-[1]">
        <RowIcon icon={item.icon} />
      </div>
      <div className="pointer-events-none relative z-[1] min-w-0">
        <div
          className={`truncate text-[13.5px] leading-tight font-medium transition-colors duration-150 ${
            selected ? "text-fg" : "text-fg/90"
          }`}
        >
          {item.title}
        </div>
        {item.subtitle ? (
          <div className="mt-[3px] truncate text-[11.5px] leading-tight text-fg-tertiary">
            {item.subtitle}
          </div>
        ) : null}
      </div>
      <div className="relative z-10 flex items-center gap-0.5">
        {canReorder && appId ? (
          <button
            type="button"
            aria-label={`Reorder ${item.title}. Use Up and Down arrow keys.`}
            title={`Reorder ${item.title}`}
            tabIndex={selected ? 0 : -1}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              onStartPinDrag(item, event.pointerId, event.clientX, event.clientY);
            }}
            onPointerMove={(event) => {
              if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
              onUpdatePinDrag(event.pointerId, event.clientX, event.clientY);
            }}
            onPointerUp={(event) => {
              if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                event.currentTarget.releasePointerCapture(event.pointerId);
              }
              onFinishPinDrag(event.pointerId);
            }}
            onPointerCancel={(event) => {
              onCancelPinDrag(event.pointerId);
            }}
            onKeyDown={(event) => {
              if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
              event.preventDefault();
              event.stopPropagation();
              onMovePin(appId, event.key === "ArrowUp" ? -1 : 1);
            }}
            className={`focus-ring grid h-7 w-7 touch-none place-items-center rounded-[7px] text-fg-quiet transition-[color,background-color] duration-100 hover:bg-surface-hover hover:text-fg ${
              draggedPin === appId ? "cursor-grabbing bg-accent-soft text-accent" : "cursor-grab"
            }`}
          >
            <GripVertical className="h-3.5 w-3.5" />
          </button>
        ) : null}
        {item.appId ? (
          <button
            type="button"
            aria-label={`${pinned ? "Unpin" : "Pin"} ${item.title}`}
            aria-pressed={pinned}
            title={`${pinned ? "Unpin" : "Pin"} ${item.title}`}
            tabIndex={selected ? 0 : -1}
            onClick={() => onTogglePin(item)}
            className={`focus-ring grid h-7 w-7 cursor-pointer place-items-center rounded-[7px] opacity-0 transition-[opacity,color,background-color] duration-100 group-hover:opacity-100 focus:opacity-100 ${
              pinned ? "bg-accent-soft text-accent" : "text-fg-tertiary hover:bg-surface-hover hover:text-fg"
            }`}
          >
            {pinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
          </button>
        ) : selected && item.id.startsWith("calc::") ? (
          <span className="text-[12px] font-semibold text-accent tabular-nums">Enter to copy</span>
        ) : selected && item.id.startsWith("file::") ? (
          <span className="text-[11.5px] font-medium text-fg-tertiary">Enter to open</span>
        ) : null}
      </div>
    </li>
  );
}
