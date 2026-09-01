import {
  ChevronDown,
  FolderSync,
  GripVertical,
  Pin,
  PinOff,
  RefreshCw,
  Search,
  Settings2,
  X,
} from "lucide-react";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { PowerMenu } from "../../components/PowerMenu";
import { displayShortcut } from "../../components/SettingsSheet";
import { IconButton, Kbd, RowIcon, SectionLabel } from "../../components/ui";
import type { PaletteItem, QuickAccessKind } from "../../lib/types";
import {
  PINNED_APP_LIMIT,
  reorderAppGroups,
  reorderPinnedApps,
  reorderQuickAccess,
  reorderSections,
} from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";
import { UpdateControl } from "../updater/UpdateControl";
import { type ContextMenuPosition, clampContextMenuPosition, ResultContextMenu } from "./ResultContextMenu";

interface ReorderDragState {
  item: PaletteItem;
  pointerId: number;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  targetItemId: string | null;
}

/** Any action that justifies opening the result context menu. */
function hasResultActions(item: PaletteItem | undefined): item is PaletteItem {
  return Boolean(item && resultActionCount(item) > 0);
}

/** Rows the context menu will render; menu viewport clamping sizes from it. */
function resultActionCount(item: PaletteItem): number {
  return (
    (item.openLocation ? 1 : 0) +
    (item.runAsAdmin ? 1 : 0) +
    (item.toggleTaskbarPin ? 1 : 0) +
    (item.showProperties ? 1 : 0)
  );
}

interface ResultMenuState {
  item: PaletteItem;
  position: ContextMenuPosition;
}

interface CategoryDragState {
  kind: "section" | "group";
  id: string;
  label: string;
  pointerId: number;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  targetId: string | null;
}

export function Palette() {
  const palette = usePalette();
  const app = useApp();
  const { settings, setOpenSettings, updateSettings, showToast } = app;

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const reorderDragRef = useRef<ReorderDragState | null>(null);
  const pendingReorderDragRef = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const reorderDragFrameRef = useRef<number | null>(null);
  const [reorderDrag, setReorderDrag] = useState<ReorderDragState | null>(null);
  const categoryDragRef = useRef<CategoryDragState | null>(null);
  const pendingCategoryDragRef = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const categoryDragFrameRef = useRef<number | null>(null);
  const [categoryDrag, setCategoryDrag] = useState<CategoryDragState | null>(null);
  const [resultMenu, setResultMenu] = useState<ResultMenuState | null>(null);
  const [previewLeaving, setPreviewLeaving] = useState(false);
  const previewLeaveTimerRef = useRef<number | null>(null);
  const leavingDragRef = useRef<ReorderDragState | null>(null);

  const closeResultMenu = useCallback((restoreFocus: boolean) => {
    setResultMenu(null);
    if (restoreFocus) requestAnimationFrame(() => inputRef.current?.focus());
  }, []);

  const openResultMenu = useCallback(
    (item: PaletteItem, index: number, x: number, y: number) => {
      const actionCount = resultActionCount(item);
      if (actionCount === 0) return;
      palette.select(index);
      const menuHeight = actionCount * 36 + 8;
      setResultMenu({ item, position: clampContextMenuPosition(x, y, menuHeight) });
    },
    [palette.select],
  );

  const togglePin = useCallback(
    (item: PaletteItem) => {
      const appId = item.appId;
      if (!appId) return;
      const pinned = settings.pinnedApps.includes(appId);
      if (pinned) {
        updateSettings({
          pinnedApps: settings.pinnedApps.filter((candidate) => candidate !== appId),
        });
        showToast("Unpinned", item.title);
        requestAnimationFrame(() => inputRef.current?.focus());
        return;
      }
      if (settings.pinnedApps.length >= PINNED_APP_LIMIT) {
        showToast("Pin limit reached", `Unpin an app before adding ${item.title}`);
        return;
      }
      updateSettings({
        pinnedApps: [...settings.pinnedApps, appId],
      });
      showToast("Pinned", item.title);
      requestAnimationFrame(() => inputRef.current?.focus());
    },
    [settings.pinnedApps, showToast, updateSettings],
  );

  const toggleAppGroup = useCallback(
    (groupId: string) => {
      app.updateSettings({
        appGroups: settings.appGroups.map((group) =>
          group.id === groupId ? { ...group, collapsed: !group.collapsed } : group,
        ),
      });
    },
    [app, settings.appGroups],
  );

  const updateCategoryDragState = useCallback((next: CategoryDragState | null) => {
    categoryDragRef.current = next;
    setCategoryDrag(next);
  }, []);

  const applyCategoryDrag = useCallback(
    (pointerId: number, x: number, y: number) => {
      const current = categoryDragRef.current;
      if (!current || current.pointerId !== pointerId) return;
      const active = current.active || Math.hypot(x - current.startX, y - current.startY) >= 4;
      const selector = current.kind === "section" ? "[data-palette-section-id]" : "[data-palette-group-id]";
      const hovered = active ? document.elementFromPoint(x, y)?.closest<HTMLElement>(selector) : null;
      const hoveredId =
        current.kind === "section" ? hovered?.dataset.paletteSectionId : hovered?.dataset.paletteGroupId;
      const targetId = hoveredId && hoveredId !== current.id ? hoveredId : null;
      updateCategoryDragState({ ...current, x, y, active, targetId: targetId ?? null });
    },
    [updateCategoryDragState],
  );

  const updateCategoryDrag = useCallback(
    (pointerId: number, x: number, y: number) => {
      pendingCategoryDragRef.current = { pointerId, x, y };
      if (categoryDragFrameRef.current !== null) return;
      categoryDragFrameRef.current = requestAnimationFrame(() => {
        categoryDragFrameRef.current = null;
        const pending = pendingCategoryDragRef.current;
        pendingCategoryDragRef.current = null;
        if (pending) applyCategoryDrag(pending.pointerId, pending.x, pending.y);
      });
    },
    [applyCategoryDrag],
  );

  const finishCategoryDrag = useCallback(
    (pointerId: number) => {
      if (categoryDragFrameRef.current !== null) {
        cancelAnimationFrame(categoryDragFrameRef.current);
        categoryDragFrameRef.current = null;
      }
      const pending = pendingCategoryDragRef.current;
      pendingCategoryDragRef.current = null;
      if (pending) applyCategoryDrag(pending.pointerId, pending.x, pending.y);
      const current = categoryDragRef.current;
      if (!current || current.pointerId !== pointerId) return;
      if (current.active && current.targetId) {
        if (current.kind === "section") {
          app.updateSettings({
            sectionOrder: reorderSections(settings.sectionOrder, current.id, current.targetId),
          });
        } else {
          app.updateSettings({
            appGroups: reorderAppGroups(settings.appGroups, current.id, current.targetId),
          });
        }
      }
      updateCategoryDragState(null);
    },
    [app, applyCategoryDrag, settings.appGroups, settings.sectionOrder, updateCategoryDragState],
  );

  const cancelCategoryDrag = useCallback(
    (pointerId: number) => {
      pendingCategoryDragRef.current = null;
      if (categoryDragFrameRef.current !== null) {
        cancelAnimationFrame(categoryDragFrameRef.current);
        categoryDragFrameRef.current = null;
      }
      if (categoryDragRef.current?.pointerId === pointerId) updateCategoryDragState(null);
    },
    [updateCategoryDragState],
  );

  const startCategoryDrag = useCallback(
    (kind: CategoryDragState["kind"], id: string, label: string, pointerId: number, x: number, y: number) => {
      updateCategoryDragState({
        kind,
        id,
        label,
        pointerId,
        startX: x,
        startY: y,
        x,
        y,
        active: false,
        targetId: null,
      });
    },
    [updateCategoryDragState],
  );

  const categoryDragHandlers = useCallback(
    (kind: CategoryDragState["kind"], id: string, label: string) => ({
      onPointerDown: (event: React.PointerEvent) => {
        if (event.button !== 0 || palette.query.trim() !== "") return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        startCategoryDrag(kind, id, label, event.pointerId, event.clientX, event.clientY);
      },
      onPointerMove: (event: React.PointerEvent) => {
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          updateCategoryDrag(event.pointerId, event.clientX, event.clientY);
        }
      },
      onPointerUp: (event: React.PointerEvent) => {
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
        finishCategoryDrag(event.pointerId);
      },
      onPointerCancel: (event: React.PointerEvent) => cancelCategoryDrag(event.pointerId),
    }),
    [cancelCategoryDrag, finishCategoryDrag, palette.query, startCategoryDrag, updateCategoryDrag],
  );

  const reorderItem = useCallback(
    (source: PaletteItem, targetItemId: string) => {
      if (source.appId && targetItemId.startsWith("app:")) {
        const pinnedApps = reorderPinnedApps(settings.pinnedApps, source.appId, targetItemId.slice(4));
        if (!pinnedApps.every((appId, index) => appId === settings.pinnedApps[index])) {
          app.updateSettings({ pinnedApps });
        }
        return;
      }
      if (source.quickAccessKind && targetItemId.startsWith("quick:")) {
        const quickAccess = reorderQuickAccess(
          settings.quickAccess,
          source.quickAccessKind,
          targetItemId.slice(6) as QuickAccessKind,
        );
        if (!quickAccess.every((kind, index) => kind === settings.quickAccess[index])) {
          app.updateSettings({ quickAccess });
        }
      }
    },
    [app, settings.pinnedApps, settings.quickAccess],
  );

  const moveItem = useCallback(
    (item: PaletteItem, direction: -1 | 1) => {
      if (item.appId) {
        const index = settings.pinnedApps.indexOf(item.appId);
        const targetAppId = settings.pinnedApps[index + direction];
        if (index >= 0 && targetAppId) reorderItem(item, `app:${targetAppId}`);
        return;
      }
      if (item.quickAccessKind) {
        const index = settings.quickAccess.indexOf(item.quickAccessKind);
        const targetKind = settings.quickAccess[index + direction];
        if (index >= 0 && targetKind) reorderItem(item, `quick:${targetKind}`);
      }
    },
    [reorderItem, settings.pinnedApps, settings.quickAccess],
  );

  const updateReorderDragState = useCallback((next: ReorderDragState | null) => {
    reorderDragRef.current = next;
    setReorderDrag(next);
  }, []);

  const startReorderDrag = useCallback(
    (item: PaletteItem, pointerId: number, x: number, y: number) => {
      updateReorderDragState({
        item,
        pointerId,
        startX: x,
        startY: y,
        x,
        y,
        active: false,
        targetItemId: null,
      });
    },
    [updateReorderDragState],
  );

  const applyReorderDrag = useCallback(
    (pointerId: number, x: number, y: number) => {
      const current = reorderDragRef.current;
      if (!current || current.pointerId !== pointerId) return;

      const active = current.active || Math.hypot(x - current.startX, y - current.startY) >= 4;
      const hoveredRow = active
        ? document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-reorder-item-id]")
        : null;
      const hoveredItemId = hoveredRow?.dataset.reorderItemId ?? null;
      const sourceItemId = reorderItemId(current.item);
      const sameGroup = hoveredItemId?.split(":", 1)[0] === sourceItemId?.split(":", 1)[0];
      const targetItemId = hoveredItemId !== sourceItemId && sameGroup ? hoveredItemId : null;
      updateReorderDragState({ ...current, x, y, active, targetItemId });
    },
    [updateReorderDragState],
  );

  const updateReorderDrag = useCallback(
    (pointerId: number, x: number, y: number) => {
      pendingReorderDragRef.current = { pointerId, x, y };
      if (reorderDragFrameRef.current !== null) return;
      reorderDragFrameRef.current = requestAnimationFrame(() => {
        reorderDragFrameRef.current = null;
        const pending = pendingReorderDragRef.current;
        pendingReorderDragRef.current = null;
        if (pending) applyReorderDrag(pending.pointerId, pending.x, pending.y);
      });
    },
    [applyReorderDrag],
  );

  const animatePreviewOut = useCallback(() => {
    const current = reorderDragRef.current;
    if (!current?.active) return;
    leavingDragRef.current = current;
    setPreviewLeaving(true);
    if (previewLeaveTimerRef.current !== null) window.clearTimeout(previewLeaveTimerRef.current);
    previewLeaveTimerRef.current = window.setTimeout(() => {
      previewLeaveTimerRef.current = null;
      leavingDragRef.current = null;
      setPreviewLeaving(false);
    }, 110);
  }, []);

  const finishReorderDrag = useCallback(
    (pointerId: number) => {
      if (reorderDragFrameRef.current !== null) {
        cancelAnimationFrame(reorderDragFrameRef.current);
        reorderDragFrameRef.current = null;
      }
      const pending = pendingReorderDragRef.current;
      pendingReorderDragRef.current = null;
      if (pending) applyReorderDrag(pending.pointerId, pending.x, pending.y);
      const current = reorderDragRef.current;
      if (!current || current.pointerId !== pointerId) return false;
      if (current.active && current.targetItemId) {
        reorderItem(current.item, current.targetItemId);
      }
      if (current.active) animatePreviewOut();
      updateReorderDragState(null);
      return current.active;
    },
    [applyReorderDrag, animatePreviewOut, reorderItem, updateReorderDragState],
  );

  const cancelReorderDrag = useCallback(
    (pointerId: number) => {
      pendingReorderDragRef.current = null;
      if (reorderDragFrameRef.current !== null) {
        cancelAnimationFrame(reorderDragFrameRef.current);
        reorderDragFrameRef.current = null;
      }
      if (reorderDragRef.current?.pointerId === pointerId) {
        if (reorderDragRef.current.active) animatePreviewOut();
        updateReorderDragState(null);
      }
    },
    [animatePreviewOut, updateReorderDragState],
  );

  useEffect(() => {
    if (!reorderDrag && !categoryDrag) return;
    const cancelOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        updateReorderDragState(null);
        updateCategoryDragState(null);
      }
    };
    window.addEventListener("keydown", cancelOnEscape);
    return () => window.removeEventListener("keydown", cancelOnEscape);
  }, [categoryDrag, reorderDrag, updateCategoryDragState, updateReorderDragState]);

  useEffect(
    () => () => {
      if (reorderDragFrameRef.current !== null) cancelAnimationFrame(reorderDragFrameRef.current);
      if (categoryDragFrameRef.current !== null) cancelAnimationFrame(categoryDragFrameRef.current);
      if (previewLeaveTimerRef.current !== null) window.clearTimeout(previewLeaveTimerRef.current);
    },
    [],
  );

  /* Keyboard-first navigation. */
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
        e.preventDefault();
        const item = palette.flatItems[palette.selected];
        if (!hasResultActions(item)) return;
        const row = document.getElementById(`prism-opt-${palette.selected}`);
        const bounds = row?.getBoundingClientRect();
        openResultMenu(
          item,
          palette.selected,
          bounds ? bounds.right - 12 : window.innerWidth / 2,
          bounds ? bounds.top + 12 : window.innerHeight / 2,
        );
        return;
      }
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
    [palette, app.openSettings, setOpenSettings, openResultMenu],
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
          <Kbd>{displayShortcut(settings.shortcut)}</Kbd>
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
        {palette.sections.length === 0 ? (
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
            return palette.sections.map((section) => {
              const sectionReorderable = palette.query.trim() === "";
              const sectionDropTarget =
                categoryDrag?.kind === "section" && categoryDrag.targetId === section.id;
              const sectionDragTargetProps = sectionReorderable
                ? { "data-palette-section-id": section.id }
                : {};
              return (
                <div
                  key={section.id}
                  {...sectionDragTargetProps}
                  data-category-drop-target={sectionDropTarget || undefined}
                  data-category-dragging={
                    categoryDrag?.kind === "section" && categoryDrag.id === section.id ? true : undefined
                  }
                  className={sectionDropTarget ? "section-category-drop" : undefined}
                >
                  {section.collapsible ? (
                    <div className="flex items-center gap-1 px-3.5 pb-1.5 pt-4">
                      <button
                        type="button"
                        aria-expanded={!section.collapsed}
                        aria-controls={`prism-section-${section.id}`}
                        onClick={() =>
                          app.updateSettings({ quickAccessCollapsed: !settings.quickAccessCollapsed })
                        }
                        className="focus-ring group/section flex min-w-0 flex-1 cursor-pointer items-center gap-1 rounded-[6px] text-left text-[11px] font-semibold text-fg-quiet uppercase hover:text-fg-secondary"
                      >
                        <ChevronDown
                          className={`h-3.5 w-3.5 transition-transform duration-150 ${
                            section.collapsed ? "-rotate-90" : "rotate-0"
                          }`}
                        />
                        <span className="truncate">{section.label}</span>
                      </button>
                      {sectionReorderable ? (
                        <button
                          type="button"
                          aria-label={`Reorder ${section.label} section`}
                          title={`Reorder ${section.label}`}
                          className="focus-ring grid h-7 w-7 shrink-0 touch-none cursor-grab place-items-center rounded-[7px] text-fg-quiet hover:bg-surface-hover hover:text-fg active:cursor-grabbing"
                          {...categoryDragHandlers("section", section.id, section.label)}
                        >
                          <GripVertical className="h-3.5 w-3.5" />
                        </button>
                      ) : null}
                    </div>
                  ) : (
                    <div className="flex items-center gap-1">
                      <div className="min-w-0 flex-1">
                        <SectionLabel>{section.label}</SectionLabel>
                      </div>
                      {sectionReorderable ? (
                        <button
                          type="button"
                          aria-label={`Reorder ${section.label} section`}
                          title={`Reorder ${section.label}`}
                          className="focus-ring mr-3.5 grid h-7 w-7 shrink-0 touch-none cursor-grab place-items-center rounded-[7px] text-fg-quiet hover:bg-surface-hover hover:text-fg active:cursor-grabbing"
                          {...categoryDragHandlers("section", section.id, section.label)}
                        >
                          <GripVertical className="h-3.5 w-3.5" />
                        </button>
                      ) : null}
                    </div>
                  )}
                  {section.groups?.map((group, groupIndex) => (
                    <div
                      key={group.id}
                      data-palette-group-id={palette.query.trim() === "" ? group.groupId : undefined}
                      data-category-drop-target={
                        categoryDrag?.kind === "group" && categoryDrag.targetId === group.groupId
                          ? true
                          : undefined
                      }
                      data-category-dragging={
                        categoryDrag?.kind === "group" && categoryDrag.id === group.groupId ? true : undefined
                      }
                      className={`${groupIndex > 0 ? "mt-1 border-t border-line pt-1" : ""} ${
                        categoryDrag?.kind === "group" && categoryDrag.targetId === group.groupId
                          ? "section-category-drop"
                          : ""
                      }`}
                    >
                      <div className="flex items-center gap-1 px-3.5 pb-1.5 pt-2">
                        <button
                          type="button"
                          aria-expanded={!group.collapsed}
                          aria-controls={`prism-section-${group.id}`}
                          onClick={() => toggleAppGroup(group.groupId)}
                          className="focus-ring group/section flex min-w-0 flex-1 cursor-pointer items-center gap-1 rounded-[6px] text-left text-[11px] font-semibold text-fg-secondary hover:text-fg"
                        >
                          <ChevronDown
                            className={`h-3.5 w-3.5 transition-transform duration-150 ${
                              group.collapsed ? "-rotate-90" : "rotate-0"
                            }`}
                          />
                          <span className="truncate">{group.label}</span>
                        </button>
                        {palette.query.trim() === "" ? (
                          <button
                            type="button"
                            aria-label={`Reorder ${group.label} group`}
                            title={`Reorder ${group.label}`}
                            className="focus-ring grid h-7 w-7 shrink-0 touch-none cursor-grab place-items-center rounded-[7px] text-fg-quiet hover:bg-surface-hover hover:text-fg active:cursor-grabbing"
                            {...categoryDragHandlers("group", group.groupId, group.label)}
                          >
                            <GripVertical className="h-3.5 w-3.5" />
                          </button>
                        ) : null}
                      </div>
                      <ul
                        id={`prism-section-${group.id}`}
                        aria-label={group.label}
                        className="m-0 flex list-none flex-col gap-[2px] p-0"
                      >
                        {group.items.map((item) => {
                          const index = flat++;
                          return renderResultRow(item, index, false, false);
                        })}
                      </ul>
                    </div>
                  ))}
                  {section.groups && section.groups.length > 0 && section.items.length > 0 ? (
                    <div className="mt-2 flex items-center gap-2 px-3.5 pb-1 pt-2 text-[10px] font-semibold text-fg-quiet uppercase">
                      <span className="h-px flex-1 bg-line" aria-hidden="true" />
                      <span>Other apps</span>
                      <span className="h-px flex-1 bg-line" aria-hidden="true" />
                    </div>
                  ) : null}
                  <ul
                    id={`prism-section-${section.id}`}
                    aria-label={section.groups?.length ? "Other apps" : section.label}
                    className="m-0 flex list-none flex-col gap-[2px] p-0"
                  >
                    {section.items.map((item) => {
                      const index = flat++;
                      const reorderable = section.id === "pinned" || section.id === "quick";
                      return renderResultRow(item, index, reorderable, section.id === "recent");
                    })}
                  </ul>
                </div>
              );
            });

            function renderResultRow(
              item: PaletteItem,
              index: number,
              reorderable: boolean,
              removable: boolean,
            ) {
              return (
                <ResultRow
                  key={item.id}
                  item={item}
                  index={index}
                  selected={palette.selected === index}
                  pinned={item.appId ? settings.pinnedApps.includes(item.appId) : false}
                  reorderable={reorderable}
                  draggedItem={reorderDrag?.active ? reorderItemId(reorderDrag.item) : null}
                  dropTargetItem={reorderDrag?.active ? reorderDrag.targetItemId : null}
                  onSelect={palette.select}
                  onRun={palette.runItem}
                  onOpenContextMenu={openResultMenu}
                  onTogglePin={togglePin}
                  removable={removable}
                  onRemoveHistory={app.removeHistory}
                  onMoveItem={moveItem}
                  onStartReorderDrag={startReorderDrag}
                  onUpdateReorderDrag={updateReorderDrag}
                  onFinishReorderDrag={finishReorderDrag}
                  onCancelReorderDrag={cancelReorderDrag}
                />
              );
            }
          })()
        )}
      </div>

      {resultMenu ? (
        <ResultContextMenu
          item={resultMenu.item}
          position={resultMenu.position}
          onOpenLocation={() => {
            const { item } = resultMenu;
            setResultMenu(null);
            item.openLocation?.();
          }}
          onRunAsAdmin={() => {
            const { item } = resultMenu;
            setResultMenu(null);
            palette.runItemAsAdmin(item);
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

      {reorderDrag?.active ? (
        <ReorderDragPreview drag={reorderDrag} />
      ) : previewLeaving && leavingDragRef.current ? (
        <ReorderDragPreview drag={leavingDragRef.current} leaving />
      ) : null}

      {categoryDrag?.active ? <CategoryDragPreview drag={categoryDrag} /> : null}

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
            <span
              className="flex items-center gap-1.5 text-[11px] text-fg-quiet"
              title={
                palette.volumes.length > 0
                  ? palette.volumes
                      .map((v) => `${v.drive}: ${v.state} (${v.indexedCount.toLocaleString()} items)`)
                      .join(", ")
                  : "Indexing file catalog..."
              }
            >
              <RefreshCw className="h-3 w-3 animate-spin" />
              indexing files
            </span>
          )}
          {palette.appsError && palette.query === "" && (
            <span className="text-[11px] text-danger">apps failed to load</span>
          )}
          <IconButton
            label="Rebuild file index"
            onClick={palette.rebuildIndex}
            disabled={palette.fileIndexing}
          >
            <FolderSync className="h-3.5 w-3.5" />
          </IconButton>
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

function reorderItemId(item: PaletteItem): string | null {
  if (item.appId) return `app:${item.appId}`;
  if (item.quickAccessKind) return `quick:${item.quickAccessKind}`;
  return null;
}

function ReorderDragPreview({ drag, leaving }: { drag: ReorderDragState; leaving?: boolean }) {
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
      <div className={`reorder-drag-preview${leaving ? " reorder-drag-preview-exit" : ""}`}>
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

function CategoryDragPreview({ drag }: { drag: CategoryDragState }) {
  const previewWidth = 208;
  const previewHeight = 48;
  const x = Math.min(Math.max(8, drag.x + 14), window.innerWidth - previewWidth - 8);
  const y = Math.min(Math.max(8, drag.y - previewHeight / 2), window.innerHeight - previewHeight - 8);

  return (
    <div
      aria-hidden="true"
      className="pointer-events-none fixed top-0 left-0 z-50 will-change-transform"
      style={{ transform: `translate3d(${x}px, ${y}px, 0)` }}
    >
      <div className="category-drag-preview">
        <span className="grid h-8 w-8 shrink-0 place-items-center rounded-[8px] bg-accent-soft text-accent">
          <GripVertical className="h-4 w-4" />
        </span>
        <span className="min-w-0">
          <span className="block truncate text-[12px] font-semibold text-fg">{drag.label}</span>
          <span className="mt-0.5 block text-[10.5px] text-fg-tertiary">
            {drag.kind === "section" ? "Section" : "App collection"}
          </span>
        </span>
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
          <div className="text-balance text-[13.5px] font-medium text-fg-secondary">
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
              className="focus-ring press mt-1 flex cursor-pointer items-center gap-1.5 rounded-[10px] bg-surface px-3 py-1.5 text-[12px] font-medium text-fg-secondary hover:bg-surface-hover hover:text-fg"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              Retry
            </button>
          )}
        </>
      ) : (
        <>
          <div className="text-balance text-[13.5px] font-medium text-fg-secondary">
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

const ResultRow = memo(function ResultRow({
  item,
  index,
  selected,
  pinned,
  reorderable,
  draggedItem,
  dropTargetItem,
  onSelect,
  onRun,
  onOpenContextMenu,
  onTogglePin,
  removable,
  onRemoveHistory,
  onMoveItem,
  onStartReorderDrag,
  onUpdateReorderDrag,
  onFinishReorderDrag,
  onCancelReorderDrag,
}: {
  item: PaletteItem;
  index: number;
  selected: boolean;
  pinned: boolean;
  reorderable: boolean;
  draggedItem: string | null;
  dropTargetItem: string | null;
  onSelect: (i: number) => void;
  onRun: (item: PaletteItem) => void;
  onOpenContextMenu: (item: PaletteItem, index: number, x: number, y: number) => void;
  onTogglePin: (item: PaletteItem) => void;
  removable: boolean;
  onRemoveHistory: (id: string) => void;
  onMoveItem: (item: PaletteItem, direction: -1 | 1) => void;
  onStartReorderDrag: (item: PaletteItem, pointerId: number, x: number, y: number) => void;
  onUpdateReorderDrag: (pointerId: number, x: number, y: number) => void;
  onFinishReorderDrag: (pointerId: number) => boolean;
  onCancelReorderDrag: (pointerId: number) => void;
}) {
  const itemReorderId = reorderItemId(item);
  const canReorder = reorderable && Boolean(itemReorderId);

  return (
    <li
      id={`prism-opt-${index}`}
      data-selected={selected}
      data-drop-target={canReorder && dropTargetItem === itemReorderId}
      data-dragging={canReorder && draggedItem === itemReorderId}
      data-reorder-item-id={canReorder ? (itemReorderId ?? undefined) : undefined}
      onMouseEnter={() => onSelect(index)}
      onContextMenu={(event) => {
        event.preventDefault();
        if (item.runAsAdmin || item.openLocation) {
          onOpenContextMenu(item, index, event.clientX, event.clientY);
        }
      }}
      className={`group row w-full text-left transition-colors duration-50 ${
        selected ? "bg-surface-active" : "hover:bg-surface-hover"
      }`}
    >
      <button
        type="button"
        tabIndex={-1}
        aria-label={`Open ${item.title}`}
        onClick={() => onRun(item)}
        className="focus-ring absolute inset-0 z-0 cursor-pointer rounded-[14px]"
      />
      <div className="pointer-events-none relative z-[1]">
        <div
          onPointerDown={(event) => {
            if (!canReorder || event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.setPointerCapture(event.pointerId);
            onStartReorderDrag(item, event.pointerId, event.clientX, event.clientY);
          }}
          onPointerMove={(event) => {
            if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
            onUpdateReorderDrag(event.pointerId, event.clientX, event.clientY);
          }}
          onPointerUp={(event) => {
            if (event.currentTarget.hasPointerCapture(event.pointerId)) {
              event.currentTarget.releasePointerCapture(event.pointerId);
            }
            const dragged = onFinishReorderDrag(event.pointerId);
            if (!dragged) onRun(item);
          }}
          onPointerCancel={(event) => onCancelReorderDrag(event.pointerId)}
          className={
            canReorder ? "pointer-events-auto touch-none cursor-grab active:cursor-grabbing" : undefined
          }
        >
          <RowIcon icon={item.icon} />
        </div>
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
        {canReorder ? (
          <button
            type="button"
            aria-label={`Reorder ${item.title}. Use Up and Down arrow keys.`}
            title={`Reorder ${item.title}`}
            tabIndex={selected ? 0 : -1}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              onStartReorderDrag(item, event.pointerId, event.clientX, event.clientY);
            }}
            onPointerMove={(event) => {
              if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
              onUpdateReorderDrag(event.pointerId, event.clientX, event.clientY);
            }}
            onPointerUp={(event) => {
              if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                event.currentTarget.releasePointerCapture(event.pointerId);
              }
              onFinishReorderDrag(event.pointerId);
            }}
            onPointerCancel={(event) => {
              onCancelReorderDrag(event.pointerId);
            }}
            onKeyDown={(event) => {
              if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
              event.preventDefault();
              event.stopPropagation();
              onMoveItem(item, event.key === "ArrowUp" ? -1 : 1);
            }}
            className={`focus-ring grid h-8 w-8 touch-none place-items-center rounded-[8px] text-fg-quiet transition-[color,background-color] duration-100 hover:bg-surface-hover hover:text-fg ${
              draggedItem === itemReorderId ? "cursor-grabbing bg-accent-soft text-accent" : "cursor-grab"
            }`}
          >
            <GripVertical className="h-4 w-4" />
          </button>
        ) : null}
        {item.appId ? (
          <>
            <button
              type="button"
              aria-label={`${pinned ? "Unpin" : "Pin"} ${item.title}`}
              aria-pressed={pinned}
              title={`${pinned ? "Unpin" : "Pin"} ${item.title}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => onTogglePin(item)}
              className={`focus-ring press grid h-8 w-8 cursor-pointer place-items-center rounded-[8px] opacity-0 group-hover:opacity-100 focus:opacity-100 ${
                pinned
                  ? "bg-accent-soft text-accent"
                  : "text-fg-tertiary hover:bg-surface-hover hover:text-fg"
              }`}
            >
              <span className="relative grid h-4 w-4 place-items-center" aria-hidden="true">
                <PinOff
                  className={`icon-swap absolute inset-0 h-4 w-4 ${
                    pinned ? "scale-100 opacity-100 blur-[0px]" : "scale-[0.25] opacity-0 blur-[4px]"
                  }`}
                />
                <Pin
                  className={`icon-swap h-4 w-4 ${
                    pinned ? "scale-[0.25] opacity-0 blur-[4px]" : "scale-100 opacity-100 blur-[0px]"
                  }`}
                />
              </span>
            </button>
            {removable ? (
              <button
                type="button"
                aria-label={`Remove ${item.title} from Recent`}
                title="Remove from Recent"
                tabIndex={selected ? 0 : -1}
                onClick={() => onRemoveHistory(item.id)}
                className="focus-ring press grid h-8 w-8 cursor-pointer place-items-center rounded-[8px] text-fg-tertiary opacity-60 transition-opacity duration-150 group-hover:opacity-100 focus:opacity-100 hover:bg-danger-soft hover:text-danger"
              >
                <X className="h-4 w-4" />
              </button>
            ) : null}
          </>
        ) : removable ? (
          <button
            type="button"
            aria-label={`Remove ${item.title} from Recent`}
            title="Remove from Recent"
            tabIndex={selected ? 0 : -1}
            onClick={() => onRemoveHistory(item.id)}
            className="focus-ring press grid h-8 w-8 cursor-pointer place-items-center rounded-[8px] text-fg-tertiary opacity-60 transition-opacity duration-150 group-hover:opacity-100 focus:opacity-100 hover:bg-danger-soft hover:text-danger"
          >
            <X className="h-4 w-4" />
          </button>
        ) : selected && item.id.startsWith("calc::") ? (
          <span className="text-[12px] font-semibold text-accent tabular-nums">Enter to copy</span>
        ) : selected && item.id.startsWith("file::") ? (
          <span className="text-[11.5px] font-medium text-fg-tertiary">Enter to open</span>
        ) : null}
      </div>
    </li>
  );
});
