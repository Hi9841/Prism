import { ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { onTransientUiDismiss } from "../../lib/transientUi";
import type { PaletteItem } from "../../lib/types";

const MENU_WIDTH = 196;
const MENU_HEIGHT = 44;
const VIEWPORT_MARGIN = 8;

export interface ContextMenuPosition {
  x: number;
  y: number;
}

export function clampContextMenuPosition(x: number, y: number): ContextMenuPosition {
  return {
    x: Math.min(Math.max(VIEWPORT_MARGIN, x), window.innerWidth - MENU_WIDTH - VIEWPORT_MARGIN),
    y: Math.min(Math.max(VIEWPORT_MARGIN, y), window.innerHeight - MENU_HEIGHT - VIEWPORT_MARGIN),
  };
}

export function ResultContextMenu({
  item,
  position,
  onRunAsAdmin,
  onClose,
}: {
  item: PaletteItem;
  position: ContextMenuPosition;
  onRunAsAdmin: () => void;
  onClose: (restoreFocus: boolean) => void;
}) {
  const itemRef = useRef<HTMLButtonElement>(null);
  const [closing, setClosing] = useState(false);
  const closingRef = useRef(false);
  const closeTimerRef = useRef<number | null>(null);

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
    itemRef.current?.focus();
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
        role="menu"
        aria-label={`${item.title} actions`}
        onKeyDown={(event) => {
          if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
            event.preventDefault();
            itemRef.current?.focus();
          } else if (event.key === "Escape" || event.key === "Tab") {
            event.preventDefault();
            close(true);
          }
        }}
        className={`context-menu-enter${closing ? " context-menu-exit" : ""} fixed z-50 w-[196px] overflow-hidden rounded-[8px] border border-line bg-bg-raised p-1 shadow-pop backdrop-blur-xl`}
        style={{ left: position.x, top: position.y }}
      >
        <button
          ref={itemRef}
          type="button"
          role="menuitem"
          onClick={onRunAsAdmin}
          className="focus-ring press flex h-9 w-full cursor-pointer items-center gap-2 rounded-[4px] px-2.5 text-left text-[12.5px] font-medium text-fg-secondary hover:bg-surface-hover hover:text-fg"
        >
          <ShieldCheck className="h-4 w-4 text-accent" aria-hidden="true" />
          <span>Run as administrator</span>
        </button>
      </div>
    </>
  );
}
