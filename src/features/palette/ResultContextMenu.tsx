import { ShieldCheck } from "lucide-react";
import { useEffect, useRef } from "react";
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

  useEffect(() => {
    itemRef.current?.focus();
    const closeWithPalette = () => onClose(false);
    const closeOnViewportChange = () => onClose(true);
    document.addEventListener("prism:close", closeWithPalette);
    window.addEventListener("resize", closeOnViewportChange);
    return () => {
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
        onClick={() => onClose(true)}
        onContextMenu={(event) => {
          event.preventDefault();
          onClose(true);
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
            onClose(true);
          }
        }}
        className="fixed z-50 w-[196px] overflow-hidden rounded-[8px] border border-line bg-bg-raised p-1 shadow-pop backdrop-blur-xl"
        style={{ left: position.x, top: position.y }}
      >
        <button
          ref={itemRef}
          type="button"
          role="menuitem"
          onClick={onRunAsAdmin}
          className="focus-ring flex h-9 w-full cursor-pointer items-center gap-2 rounded-[6px] px-2.5 text-left text-[12.5px] font-medium text-fg-secondary transition-colors duration-100 hover:bg-surface-hover hover:text-fg"
        >
          <ShieldCheck className="h-4 w-4 text-accent" aria-hidden="true" />
          <span>Run as administrator</span>
        </button>
      </div>
    </>
  );
}
