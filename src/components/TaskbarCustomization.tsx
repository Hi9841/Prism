import { ImageUp, LoaderCircle, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import diamondIcon from "../assets/taskbar-icons/diamond.svg";
import gemIcon from "../assets/taskbar-icons/gem.svg";
import {
  base64ToBytes,
  getTaskbarSettings,
  removeCustomStartIcon,
  selectCustomStartIcon,
  setCustomStartIcon,
  setTaskbarAutoHide,
  setTaskbarCombineButtons,
  setTaskbarStartIcon,
  setTaskbarThickness,
  type TaskbarCombineMode,
  type TaskbarSettings,
  type TaskbarStartIcon,
  type TaskbarThickness,
} from "../lib/bridge";
import { useApp } from "../state/app";
import { Segmented, SettingsRow, Toggle } from "./ui";

type BusyControl = "load" | "thickness" | "autoHide" | "combine" | "icon";

export function TaskbarCustomization() {
  const { showToast } = useApp();
  const [settings, setSettings] = useState<TaskbarSettings | null>(null);
  const [busy, setBusy] = useState<BusyControl | null>("load");

  const refresh = useCallback(async () => {
    setBusy("load");
    try {
      setSettings(await getTaskbarSettings());
    } catch (error) {
      showToast("Taskbar settings unavailable", String(error), "error");
    } finally {
      setBusy(null);
    }
  }, [showToast]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const apply = useCallback(
    async (control: BusyControl, action: () => Promise<void>) => {
      if (busy) return;
      setBusy(control);
      try {
        await action();
        setSettings(await getTaskbarSettings());
      } catch (error) {
        showToast("Taskbar not changed", String(error), "error");
      } finally {
        setBusy(null);
      }
    },
    [busy, showToast],
  );

  if (!settings) {
    if (busy !== "load") {
      return (
        <div className="flex flex-wrap items-center justify-between gap-3 text-[12px] text-fg-secondary">
          <p role="alert">Could not load taskbar settings. Retry to reconnect.</p>
          <button
            type="button"
            onClick={() => void refresh()}
            className="focus-ring press min-h-11 rounded-[9px] bg-surface px-3 font-medium text-fg hover:bg-surface-hover"
          >
            Retry taskbar settings
          </button>
        </div>
      );
    }
    return (
      <div className="flex min-h-12 items-center justify-center text-fg-tertiary" role="status">
        <LoaderCircle className="h-4 w-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        <span className="sr-only">Loading taskbar settings</span>
      </div>
    );
  }

  return (
    <>
      <SettingsRow
        title="Taskbar density"
        detail="Controls icon scale; Windows 11 keeps the taskbar surface height fixed"
      >
        <fieldset disabled={busy !== null} className="m-0 border-0 p-0 disabled:opacity-55">
          <Segmented<TaskbarThickness>
            label="Taskbar density"
            value={settings.thickness}
            onChange={(value) => void apply("thickness", () => setTaskbarThickness(value))}
            options={[
              { value: "compact", label: "Compact" },
              { value: "default", label: "Default" },
              { value: "adaptive", label: "When full" },
            ]}
          />
        </fieldset>
      </SettingsRow>

      <SettingsRow title="Auto-hide taskbar" detail="Removes the taskbar until you move to its edge">
        <Toggle
          checked={settings.autoHide}
          label="Auto-hide taskbar"
          disabled={busy !== null}
          onChange={(value) => void apply("autoHide", () => setTaskbarAutoHide(value))}
        />
      </SettingsRow>

      <SettingsRow title="Combine buttons" detail="Controls labels and task grouping">
        <fieldset disabled={busy !== null} className="m-0 border-0 p-0 disabled:opacity-55">
          <Segmented<TaskbarCombineMode>
            label="Combine taskbar buttons"
            value={settings.combineButtons}
            onChange={(value) => void apply("combine", () => setTaskbarCombineButtons(value))}
            options={[
              { value: "always", label: "Always" },
              { value: "whenFull", label: "When full" },
              { value: "never", label: "Never" },
            ]}
          />
        </fieldset>
      </SettingsRow>

      <StartIconControl settings={settings} busy={busy !== null} apply={(action) => apply("icon", action)} />
    </>
  );
}

function StartIconControl({
  settings,
  busy,
  apply,
}: {
  settings: TaskbarSettings;
  busy: boolean;
  apply: (action: () => Promise<void>) => Promise<void>;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [customUrls, setCustomUrls] = useState<Array<{ id: string; url: string }>>([]);

  useEffect(() => {
    const urls = settings.customStartIcons.map((icon) => ({
      id: icon.id,
      url: URL.createObjectURL(new Blob([base64ToBytes(icon.preview)], { type: "image/png" })),
    }));
    setCustomUrls(urls);
    return () =>
      urls.forEach(({ url }) => {
        URL.revokeObjectURL(url);
      });
  }, [settings.customStartIcons]);

  const chooseIcon = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(event.target.files ?? []);
    event.target.value = "";
    if (!files.length) return;
    await apply(async () => {
      for (const file of files) {
        if (file.size > 2 * 1024 * 1024) throw new Error("Choose PNGs smaller than 2 MB");
        if (file.type && file.type !== "image/png") throw new Error("Choose PNG images");
        await setCustomStartIcon(new Uint8Array(await file.arrayBuffer()));
      }
    });
  };

  const options: Array<{ value: TaskbarStartIcon; label: string; icon: React.ReactNode }> = [
    { value: "system", label: "System", icon: <WindowsGlyph /> },
    { value: "gem", label: "Gem", icon: <img src={gemIcon} alt="" className="h-5 w-5" /> },
    { value: "diamond", label: "Diamond", icon: <img src={diamondIcon} alt="" className="h-5 w-5" /> },
  ];

  return (
    <SettingsRow title="Start button icon" detail="Recommended: 96 x 96 PNG with a transparent background">
      <div className="flex max-w-[260px] flex-wrap justify-end gap-1.5">
        <fieldset disabled={busy} className="contents border-0 p-0 disabled:opacity-55">
          <legend className="sr-only">Start button icon</legend>
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              title={option.label}
              aria-label={`${option.label} Start button icon`}
              aria-pressed={settings.startIcon === option.value}
              onClick={() => void apply(() => setTaskbarStartIcon(option.value))}
              className={`focus-ring press grid h-12 min-w-[46px] cursor-pointer place-items-center gap-0.5 rounded-[8px] border px-2 py-1 text-[10px] font-medium ${
                settings.startIcon === option.value
                  ? "border-accent/70 bg-accent/12 text-fg"
                  : "border-transparent bg-surface text-fg-tertiary hover:bg-surface-hover hover:text-fg"
              }`}
            >
              <span className="grid h-6 w-6 place-items-center" aria-hidden="true">
                {option.icon}
              </span>
              <span>{option.label}</span>
            </button>
          ))}
          <button
            type="button"
            title="Add custom PNG"
            aria-label="Add custom Start button PNG"
            onClick={() => inputRef.current?.click()}
            className="focus-ring press grid h-12 min-w-[46px] cursor-pointer place-items-center gap-0.5 rounded-[8px] border border-transparent bg-surface px-2 py-1 text-[10px] font-medium text-fg-tertiary hover:bg-surface-hover hover:text-fg"
          >
            <span className="grid h-6 w-6 place-items-center" aria-hidden="true">
              <ImageUp className="h-4 w-4" />
            </span>
            <span>Custom</span>
          </button>
          {customUrls.map((custom, index) => {
            const selected = settings.startIcon === "custom" && settings.selectedCustomIcon === custom.id;
            return (
              <div key={custom.id} className="relative h-12 min-w-[46px]">
                <button
                  type="button"
                  title={`Custom icon ${index + 1}`}
                  aria-label={`Use custom Start button icon ${index + 1}`}
                  aria-pressed={selected}
                  onClick={() => void apply(() => selectCustomStartIcon(custom.id))}
                  className={`focus-ring press grid h-12 min-w-[46px] cursor-pointer place-items-center gap-0.5 rounded-[8px] border px-2 py-1 text-[10px] font-medium ${
                    selected
                      ? "border-accent/70 bg-accent/12 text-fg"
                      : "border-transparent bg-surface text-fg-tertiary hover:bg-surface-hover hover:text-fg"
                  }`}
                >
                  <span className="grid h-6 w-6 place-items-center" aria-hidden="true">
                    <img src={custom.url} alt="" className="h-5 w-5 object-contain" />
                  </span>
                  <span>Custom {index + 1}</span>
                </button>
                <button
                  type="button"
                  title={`Remove custom icon ${index + 1}`}
                  aria-label={`Remove custom Start button icon ${index + 1}`}
                  onClick={() => void apply(() => removeCustomStartIcon(custom.id))}
                  className="focus-ring press absolute -right-1 -top-1 grid h-5 w-5 cursor-pointer place-items-center rounded-full border border-line bg-bg-raised text-fg-tertiary shadow-sm hover:text-danger"
                >
                  <X className="h-3 w-3" aria-hidden="true" />
                </button>
              </div>
            );
          })}
        </fieldset>
        <input
          ref={inputRef}
          type="file"
          multiple
          accept="image/png,.png"
          className="sr-only"
          tabIndex={-1}
          aria-label="Choose a custom Start button PNG"
          onChange={chooseIcon}
        />
      </div>
    </SettingsRow>
  );
}

function WindowsGlyph() {
  return (
    <span className="grid h-4 w-4 grid-cols-2 gap-[1.5px]" aria-hidden="true">
      <span className="bg-fg-tertiary" />
      <span className="bg-fg-tertiary" />
      <span className="bg-fg-tertiary" />
      <span className="bg-fg-tertiary" />
    </span>
  );
}
