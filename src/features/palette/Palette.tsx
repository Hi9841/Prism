import { Pin, PinOff, RefreshCw, Search, Settings2 } from "lucide-react";
import { useCallback, useEffect, useRef } from "react";
import { PowerMenu } from "../../components/PowerMenu";
import { IconButton, Kbd, RowIcon, SectionLabel } from "../../components/ui";
import type { PaletteItem } from "../../lib/types";
import { PINNED_APP_LIMIT } from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";

export function Palette() {
  const palette = usePalette();
  const app = useApp();
  const { settings, setOpenSettings } = app;

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

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
                    return (
                      <ResultRow
                        key={item.id}
                        item={item}
                        index={index}
                        selected={palette.selected === index}
                        pinned={item.appId ? settings.pinnedApps.includes(item.appId) : false}
                        onSelect={palette.select}
                        onRun={palette.runItem}
                        onTogglePin={togglePin}
                      />
                    );
                  })}
                </ul>
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
          <PowerMenu />
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
  pinned,
  onSelect,
  onRun,
  onTogglePin,
}: {
  item: PaletteItem;
  index: number;
  selected: boolean;
  pinned: boolean;
  onSelect: (i: number) => void;
  onRun: (item: PaletteItem) => void;
  onTogglePin: (item: PaletteItem) => void;
}) {
  return (
    <li
      id={`prism-opt-${index}`}
      data-selected={selected}
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
      <div className="relative z-10 flex items-center">
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
