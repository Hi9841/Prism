import { type ReactNode, useRef } from "react";
import { hueForName, nameToMonogram } from "../lib/emoji";
import type { PaletteIcon } from "../lib/types";

/* ---------------- Settings row ---------------- */

export function SettingsRow({
  title,
  detail,
  children,
}: {
  title: string;
  detail?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-3">
      <div className="min-w-0 flex-1 basis-40">
        <div className="text-[13px] font-medium text-fg">{title}</div>
        {detail ? <div className="mt-0.5 text-[11.5px] text-fg-tertiary">{detail}</div> : null}
      </div>
      <div className="min-w-0 max-w-full shrink-0">{children}</div>
    </div>
  );
}

/* ---------------- Kbd ---------------- */

export function Kbd({ children }: { children: ReactNode }) {
  return <span className="kbd">{children}</span>;
}

/* ---------------- Row icon ---------------- */

export function RowIcon({ icon, size = 40 }: { icon: PaletteIcon; size?: number }) {
  const style = { width: size, height: size };
  switch (icon.kind) {
    case "tile": {
      const Icon = icon.icon;
      return (
        <div className={`tile tile-${icon.tint}`} style={style}>
          <Icon className="text-white/95" strokeWidth={2} />
        </div>
      );
    }
    case "emoji":
      return (
        <div className="grid shrink-0 place-items-center" style={{ width: size, height: size }}>
          <span className="text-[22px] leading-none">{icon.char}</span>
        </div>
      );
    case "app":
      return icon.icon ? (
        <AppLogo src={icon.icon} name={icon.name} size={size} />
      ) : (
        <Monogram name={icon.name} size={size} />
      );
  }
}

/* ---------------- App logo tile ---------------- */

export function AppLogo({ src, name, size = 40 }: { src: string; name: string; size?: number }) {
  return (
    <div
      className="grid shrink-0 place-items-center rounded-[10px] bg-surface"
      style={{ width: size, height: size }}
      role="img"
      aria-label={name}
    >
      <img
        src={src}
        alt=""
        draggable={false}
        className="select-none object-contain"
        style={{ width: size - 8, height: size - 8 }}
      />
    </div>
  );
}

/* ---------------- Monogram tile ---------------- */

export function Monogram({ name, size = 40 }: { name: string; size?: number }) {
  const hue = hueForName(name);
  return (
    <div
      className="tile tile-mono"
      style={
        {
          width: size,
          height: size,
          "--mono-h": `${0.38 + hue * 0.34}`,
        } as React.CSSProperties
      }
    >
      <span className="font-semibold text-white/90" style={{ fontSize: Math.max(11, size * 0.34) }}>
        {nameToMonogram(name)}
      </span>
    </div>
  );
}

/* ---------------- Section label ---------------- */

export function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div className="px-3.5 pb-1.5 pt-4 text-[11px] font-semibold text-fg-quiet uppercase">{children}</div>
  );
}

/* ---------------- Toggle ---------------- */

export function Toggle({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="focus-ring relative h-[24px] w-[40px] cursor-pointer rounded-full transition-colors duration-200 disabled:cursor-default disabled:opacity-45"
      style={{ background: checked ? "var(--accent)" : "var(--t-track)" }}
    >
      <span
        className="absolute top-[3px] left-[3px] block h-[18px] w-[18px] rounded-full bg-white shadow-sm transition-transform duration-150 [transition-timing-function:var(--ease-out-soft)]"
        style={{ transform: checked ? "translateX(16px)" : "translateX(0)" }}
      />
    </button>
  );
}

/* ---------------- Segmented ---------------- */

export function Segmented<T extends string | number>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label?: string;
}) {
  const fieldsetRef = useRef<HTMLFieldSetElement>(null);
  const activeIndex = Math.max(
    0,
    options.findIndex((option) => option.value === value),
  );

  const selectWithKeyboard = (event: React.KeyboardEvent<HTMLFieldSetElement>) => {
    let nextIndex = activeIndex;
    if (event.key === "ArrowLeft") nextIndex = (activeIndex - 1 + options.length) % options.length;
    else if (event.key === "ArrowRight") nextIndex = (activeIndex + 1) % options.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = options.length - 1;
    else return;

    event.preventDefault();
    onChange(options[nextIndex].value);
    fieldsetRef.current?.querySelectorAll<HTMLButtonElement>("[data-segment-option]")[nextIndex]?.focus();
  };

  return (
    <fieldset
      ref={fieldsetRef}
      aria-label={label}
      onKeyDown={selectWithKeyboard}
      className="relative inline-grid rounded-[10px] bg-surface p-[3px] shadow-[inset_0_1px_0_rgb(255_255_255/0.04)]"
      style={{ gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))` }}
    >
      <span
        aria-hidden="true"
        className="pointer-events-none absolute top-[3px] bottom-[3px] left-[3px] rounded-[7px] bg-seg-active shadow-[0_1px_3px_rgb(0_0_0/0.25),inset_0_1px_0_rgb(255_255_255/0.08)] transition-transform duration-200 [transition-timing-function:var(--ease-out-soft)]"
        style={{
          width: `calc((100% - 6px) / ${options.length})`,
          transform: `translateX(${activeIndex * 100}%)`,
        }}
      />
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            aria-pressed={active}
            data-segment-option
            tabIndex={active ? 0 : -1}
            onClick={() => onChange(o.value)}
            className={`focus-ring relative z-10 min-w-8 cursor-pointer rounded-[7px] px-3 py-[5px] text-[12px] font-medium transition-colors duration-150 ${
              active ? "text-fg" : "text-fg-tertiary hover:text-fg-secondary"
            }`}
          >
            {o.label}
          </button>
        );
      })}
    </fieldset>
  );
}

/* ---------------- Icon button ---------------- */

export function IconButton({
  onClick,
  label,
  children,
  active,
  disabled,
  "data-settings-trigger": settingsTrigger,
  "aria-haspopup": ariaHasPopup,
  "aria-expanded": ariaExpanded,
  "aria-controls": ariaControls,
}: {
  onClick: () => void;
  label: string;
  children: ReactNode;
  active?: boolean;
  disabled?: boolean;
  "data-settings-trigger"?: boolean;
  "aria-haspopup"?: "menu";
  "aria-expanded"?: boolean;
  "aria-controls"?: string;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-disabled={disabled}
      aria-haspopup={ariaHasPopup}
      aria-expanded={ariaExpanded}
      aria-controls={ariaControls}
      data-settings-trigger={settingsTrigger || undefined}
      onClick={onClick}
      disabled={disabled}
      className={`focus-ring grid h-8 w-8 cursor-pointer place-items-center rounded-[10px] transition-colors duration-150 ${
        disabled
          ? "cursor-default opacity-40"
          : active
            ? "bg-surface-active text-fg"
            : "text-fg-tertiary hover:bg-surface-hover hover:text-fg-secondary"
      }`}
    >
      {children}
    </button>
  );
}
