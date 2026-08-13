import { Check } from "lucide-react";
import { useApp } from "../state/app";

export function ToastStack() {
  const { toasts } = useApp();
  return (
    <div
      aria-live="polite"
      className="pointer-events-none absolute inset-x-0 bottom-4 z-50 flex flex-col items-center gap-2"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          className={`toast-enter ${t.closing ? "toast-exit" : ""} flex cursor-default items-center gap-2.5 rounded-[14px] border border-line bg-toast py-2 pl-3 pr-4 shadow-raised backdrop-blur-xl`}
          style={{ boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.05), var(--shadow-raised)" }}
        >
          <span className="grid h-5 w-5 place-items-center rounded-full bg-accent/90 text-[11px] text-accent-fg">
            <Check strokeWidth={3} className="h-3 w-3" />
          </span>
          <span className="text-[12.5px] font-medium text-fg">
            {t.title}
            {t.detail ? <span className="ml-1.5 font-normal text-fg-tertiary">{t.detail}</span> : null}
          </span>
        </div>
      ))}
    </div>
  );
}
