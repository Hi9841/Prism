import { Lock, Power, RotateCcw } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { type PowerAction, performPowerAction } from "../lib/bridge";
import { onTransientUiDismiss } from "../lib/transientUi";
import { useApp } from "../state/app";
import { IconButton } from "./ui";

const ACTIONS: { action: PowerAction; label: string; icon: typeof Power }[] = [
  { action: "lock", label: "Lock", icon: Lock },
  { action: "shutdown", label: "Shut down", icon: Power },
  { action: "restart", label: "Restart", icon: RotateCcw },
];

export function PowerMenu() {
  const { openSettings, showToast } = useApp();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<PowerAction | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  const focusItem = useCallback((index: number) => {
    const items = rootRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']");
    if (!items?.length) return;
    items[(index + items.length) % items.length]?.focus();
  }, []);

  const openMenu = useCallback(
    (focusIndex = 0) => {
      setOpen(true);
      requestAnimationFrame(() => focusItem(focusIndex));
    },
    [focusItem],
  );

  useEffect(() => {
    if (!open) return;
    const closeOnOutsidePress = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeWithPalette = () => setOpen(false);
    const offTransientDismiss = onTransientUiDismiss(closeWithPalette);
    document.addEventListener("pointerdown", closeOnOutsidePress);
    document.addEventListener("prism:close", closeWithPalette);
    return () => {
      offTransientDismiss();
      document.removeEventListener("pointerdown", closeOnOutsidePress);
      document.removeEventListener("prism:close", closeWithPalette);
    };
  }, [open]);

  useEffect(() => {
    if (openSettings) setOpen(false);
  }, [openSettings]);

  const runAction = async (action: PowerAction) => {
    if (busy) return;
    setBusy(action);
    try {
      await performPowerAction(action);
      setOpen(false);
      document.dispatchEvent(new CustomEvent("prism:close"));
    } catch (error) {
      showToast(`Could not ${action === "shutdown" ? "shut down" : action}`, String(error));
    } finally {
      setBusy(null);
    }
  };

  const onMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(rootRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']") ?? []);
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    let next: number | null = null;
    if (event.key === "ArrowDown") next = current + 1;
    else if (event.key === "ArrowUp") next = current - 1;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = items.length - 1;
    else if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
      return;
    } else {
      return;
    }
    event.preventDefault();
    focusItem(next);
  };

  return (
    <div ref={rootRef} className="relative">
      {open ? (
        <div
          id={menuId}
          role="menu"
          aria-label="Power options"
          onKeyDown={onMenuKeyDown}
          className="power-menu absolute right-0 bottom-10 z-30 w-40 overflow-hidden rounded-[8px] border border-line bg-bg-raised p-1 shadow-pop backdrop-blur-xl"
        >
          {ACTIONS.map(({ action, label, icon: ActionIcon }) => (
            <button
              key={action}
              type="button"
              role="menuitem"
              disabled={busy !== null}
              onClick={() => void runAction(action)}
              className="focus-ring flex h-9 w-full cursor-pointer items-center gap-2 rounded-[6px] px-2.5 text-left text-[12.5px] font-medium text-fg-secondary transition-colors duration-100 hover:bg-surface-hover hover:text-fg disabled:cursor-default disabled:opacity-50"
            >
              <ActionIcon
                className={`h-4 w-4 ${action === "shutdown" ? "text-danger" : "text-fg-tertiary"}`}
              />
              <span>{label}</span>
            </button>
          ))}
        </div>
      ) : null}
      <div ref={triggerRef}>
        <IconButton
          label="Power options"
          active={open}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-controls={open ? menuId : undefined}
          onClick={() => (open ? setOpen(false) : openMenu())}
        >
          <Power className="h-3.5 w-3.5" />
        </IconButton>
      </div>
    </div>
  );
}
