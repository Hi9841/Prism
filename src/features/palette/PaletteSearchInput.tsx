import { Search, Settings2 } from "lucide-react";
import { type RefObject, useCallback, useEffect } from "react";
import { IconButton, Kbd } from "../../components/ui";

interface PaletteSearchInputProps {
  inputRef: RefObject<HTMLInputElement | null>;
  query: string;
  shortcut: string;
  activeResultId?: string;
  resultCount: number;
  busy: boolean;
  settingsOpen: boolean;
  onQueryChange: (query: string) => void;
  onMove: (delta: -1 | 1) => void;
  onRunSelected: () => void;
  onOpenSelectedMenu: () => void;
  onDismiss: () => void;
  onToggleSettings: () => void;
}

export function PaletteSearchInput({
  inputRef,
  query,
  shortcut,
  activeResultId,
  resultCount,
  busy,
  settingsOpen,
  onQueryChange,
  onMove,
  onRunSelected,
  onOpenSelectedMenu,
  onDismiss,
  onToggleSettings,
}: PaletteSearchInputProps) {
  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.nativeEvent.isComposing) return;

      if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
        event.preventDefault();
        onOpenSelectedMenu();
        return;
      }

      switch (event.key) {
        case "ArrowDown":
          event.preventDefault();
          onMove(1);
          break;
        case "ArrowUp":
          event.preventDefault();
          onMove(-1);
          break;
        case "Enter":
          event.preventDefault();
          onRunSelected();
          break;
        case "Escape":
          event.preventDefault();
          onDismiss();
          break;
        case "k":
          if (event.ctrlKey || event.metaKey) {
            event.preventDefault();
            document.dispatchEvent(new CustomEvent("prism:close"));
          }
          break;
        case "Backspace":
          if (event.ctrlKey) {
            event.preventDefault();
            onQueryChange("");
          }
          break;
      }
    },
    [onDismiss, onMove, onOpenSelectedMenu, onQueryChange, onRunSelected],
  );

  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }, [inputRef]);

  return (
    <div className="px-5 pb-1 pt-5">
      <div className="search-field flex items-center gap-3 px-4 py-3">
        <Search className="h-[18px] w-[18px] shrink-0 text-fg-tertiary" strokeWidth={2} />
        <input
          ref={inputRef}
          data-prism-search
          type="text"
          role="combobox"
          aria-autocomplete="list"
          aria-haspopup="grid"
          aria-expanded={resultCount > 0}
          aria-controls="prism-results"
          aria-activedescendant={activeResultId}
          aria-busy={busy}
          aria-label="Search files, folders, or apps, or type a calculation"
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Search files, folders, and apps…"
          spellCheck={false}
          autoComplete="off"
          className="min-w-0 flex-1 bg-transparent text-[15px] font-medium text-fg outline-none placeholder:text-fg-quiet"
        />
        <Kbd>{shortcut}</Kbd>
        <IconButton label="Settings" data-settings-trigger active={settingsOpen} onClick={onToggleSettings}>
          <Settings2 className="h-4 w-4" strokeWidth={2} />
        </IconButton>
      </div>
    </div>
  );
}
