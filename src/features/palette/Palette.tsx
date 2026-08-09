import { RefreshCw, Search, Settings2 } from "lucide-react";
import { useCallback, useEffect, useRef } from "react";
import { IconButton, Kbd, RowIcon, SectionLabel } from "../../components/ui";
import type { PaletteItem } from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";

export function Palette() {
  const palette = usePalette();
  const app = useApp();
  const { settings, setOpenSettings } = app;

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  /* Keyboard-first navigation. */
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          palette.move(1);
          break;
        case "Tab":
          e.preventDefault();
          palette.move(e.shiftKey ? -1 : 1);
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
            role="combobox"
            aria-expanded="true"
            aria-controls="prism-results"
            aria-activedescendant={
              palette.selected >= 0 && palette.selected < palette.flatItems.length
                ? `prism-opt-${palette.selected}`
                : undefined
            }
            aria-autocomplete="list"
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
        role="listbox"
        id="prism-results"
        aria-label="Search results"
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
                <div className="flex flex-col gap-[2px]">
                  {section.items.map((item) => {
                    const index = flat++;
                    return (
                      <ResultRow
                        key={item.id}
                        item={item}
                        index={index}
                        selected={palette.selected === index}
                        onSelect={palette.select}
                        onRun={palette.runItem}
                      />
                    );
                  })}
                </div>
              </div>
            ));
          })()
        )}
      </div>

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
        </div>
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
  onSelect,
  onRun,
}: {
  item: PaletteItem;
  index: number;
  selected: boolean;
  onSelect: (i: number) => void;
  onRun: (item: PaletteItem) => void;
}) {
  return (
    <button
      type="button"
      role="option"
      id={`prism-opt-${index}`}
      aria-selected={selected}
      tabIndex={-1}
      data-selected={selected}
      onMouseEnter={() => onSelect(index)}
      onClick={() => onRun(item)}
      className={`row w-full cursor-pointer text-left transition-[background-color,box-shadow] duration-100 ${
        selected ? "bg-surface-active" : "hover:bg-surface-hover"
      }`}
    >
      <RowIcon icon={item.icon} />
      <div className="relative min-w-0">
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
      <div className="relative flex items-center">
        {selected && item.id.startsWith("calc::") && (
          <span className="text-[12px] font-semibold text-accent tabular-nums">Enter to copy</span>
        )}
        {selected && item.id.startsWith("file::") && (
          <span className="text-[11.5px] font-medium text-fg-tertiary">Enter to open</span>
        )}
      </div>
    </button>
  );
}
