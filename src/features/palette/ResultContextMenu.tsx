import { FolderOpen, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { onTransientUiDismiss } from "../../lib/transientUi";
import type { PaletteItem } from "../../lib/types";

const MENU_WIDTH = 196;
const ITEM_HEIGHT = 36;
const PADDING = 8;
const VIEWPORT_MARGIN = 8;

export interface ContextMenuPosition {
  x: number;
  y: number;
}

export function clampContextMenuPosition(
  x: number,
  y: number,
  menuHeight = ITEM_HEIGHT + PADDING,
): ContextMenuPosition {
  return {
    x: Math.min(Math.max(VIEWPORT_MARGIN, x), window.innerWidth - MENU_WIDTH - VIEWPORT_MARGIN),
    y: Math.min(Math.max(VIEWPORT_MARGIN, y), window.innerHeight - menuHeight - VIEWPORT_MARGIN),
  };
}

export function ResultContextMenu({
  item,
  position,
  onOpenLocation,
  onRunAsAdmin,
  onClose,
}: {
  item: PaletteItem;
  position: ContextMenuPosition;
  onOpenLocation?: () => void;
  onRunAsAdmin?: () => void;
  onClose: (restoreFocus: boolean) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [closing, setClosing] = useState(false);
  const closingRef = useRef(false);
  const closeTimerRef = useRef<number | null>(null);

  const hasOpenLocation = Boolean(item.openLocation && onOpenLocation);
  const hasRunAsAdmin = Boolean(item.runAsAdmin && onRunAsAdmin);

  // Deliberate closes animate out first; closes triggered by the palette
  // hiding (prism:close, transient dismiss) snap since the window is gone.
  const close = useCallback(
    (restoreFocus: boolean, animate = true) => {
      if (closingRef.current) return;
      if (!animate) {
        onClose(restoreFocus);
        return;
      }
      closingRef.current = true;
      setClosing(true);
      closeTimerRef.current = window.setTimeout(() => {
        closeTimerRef.current = null;
        onClose(restoreFocus);
      }, 110);
    },
    [onClose],
  );

  useEffect(
    () => () => {
      if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    // Focus first available menuitem button
    const firstButton = containerRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]');
    firstButton?.focus();

    const closeWithPalette = () => onClose(false);
    const closeOnViewportChange = () => onClose(true);
    const offTransientDismiss = onTransientUiDismiss(closeWithPalette);
    document.addEventListener("prism:close", closeWithPalette);
    window.addEventListener("resize", closeOnViewportChange);
    return () => {
      offTransientDismiss();
      document.removeEventListener("prism:close", closeWithPalette);
      window.removeEventListener("resize", closeOnViewportChange);
    };
  }, [onClose]);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const buttons = Array.from(
      containerRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
    );
    if (buttons.length === 0) return;

    const currentIndex = buttons.indexOf(document.activeElement as HTMLButtonElement);

    if (event.key === "ArrowDown") {
      event.preventDefault();
      const nextIndex = currentIndex >= 0 ? (currentIndex + 1) % buttons.length : 0;
      buttons[nextIndex]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      const nextIndex =
        currentIndex >= 0 ? (currentIndex - 1 + buttons.length) % buttons.length : buttons.length - 1;
      buttons[nextIndex]?.focus();
    } else if (event.key === "Home") {
      event.preventDefault();
      buttons[0]?.focus();
    } else if (event.key === "End") {
      event.preventDefault();
      buttons[buttons.length - 1]?.focus();
    } else if (event.key === "Escape" || event.key === "Tab") {
      event.preventDefault();
      close(true);
    }
  };

  return (
    <>
      <button
        type="button"
        tabIndex={-1}
        aria-label="Close result menu"
        className="fixed inset-0 z-40 cursor-default bg-transparent"
        onClick={() => close(true)}
        onContextMenu={(event) => {
          event.preventDefault();
          close(true);
        }}
      />
      <div
        ref={containerRef}
        role="menu"
        aria-label={`${item.title} actions`}
        onKeyDown={handleKeyDown}
        className={`context-menu-enter${closing ? " context-menu-exit" : ""} fixed z-50 w-[196px] overflow-hidden rounded-[8px] border border-line bg-bg-raised p-1 shadow-pop backdrop-blur-xl`}
        style={{ left: position.x, top: position.y }}
      >
        {hasOpenLocation && (
          <button
            type="button"
            role="menuitem"
            onClick={onOpenLocation}
            className="focus-ring press flex h-9 w-full cursor-pointer items-center gap-2 rounded-[4px] px-2.5 text-left text-[12.5px] font-medium text-fg-secondary hover:bg-surface-hover hover:text-fg"
          >
            <FolderOpen className="h-4 w-4 text-accent" aria-hidden="true" />
            <span>Open location</span>
          </button>
        )}
        {hasRunAsAdmin && (
          <button
            type="button"
            role="menuitem"
            onClick={onRunAsAdmin}
            className="focus-ring press flex h-9 w-full cursor-pointer items-center gap-2 rounded-[4px] px-2.5 text-left text-[12.5px] font-medium text-fg-secondary hover:bg-surface-hover hover:text-fg"
          >
            <ShieldCheck className="h-4 w-4 text-accent" aria-hidden="true" />
            <span>Run as administrator</span>
          </button>
        )}
      </div>
    </>
  );
}
