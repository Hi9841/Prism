import { Check, CircleAlert, X } from "lucide-react";
import { useApp } from "../state/app";

export function ToastStack() {
  const { dismissToast, toasts } = useApp();
  return (
    <div
      aria-live="polite"
      aria-atomic="true"
      className="pointer-events-none absolute inset-x-0 bottom-4 z-50 flex flex-col items-center gap-2"
    >
      {toasts.map((t) => {
        const isError = t.kind === "error";
        return (
          <div
            key={t.id}
            className={`toast-enter ${t.closing ? "toast-exit" : ""} pointer-events-auto flex min-h-11 max-w-[calc(100%_-_2rem)] cursor-default items-center gap-2.5 rounded-[14px] border bg-toast py-1.5 ps-3 shadow-raised backdrop-blur-xl ${isError ? "border-danger/35 pe-1" : "border-line pe-4"}`}
            style={{ boxShadow: "inset 0 1px 0 rgb(255 255 255 / 0.05), var(--shadow-raised)" }}
          >
            <span
              className={`grid h-5 w-5 shrink-0 place-items-center rounded-full ${isError ? "bg-danger-soft text-danger" : "bg-accent/90 text-accent-fg"}`}
              aria-hidden="true"
            >
              {isError ? (
                <CircleAlert strokeWidth={2.5} className="h-3.5 w-3.5" />
              ) : (
                <Check strokeWidth={3} className="h-3 w-3" />
              )}
            </span>
            <span className="min-w-0 text-[12.5px] font-medium text-fg">
              {t.title}
              {t.detail ? <span className="ms-1.5 font-normal text-fg-tertiary">{t.detail}</span> : null}
            </span>
            {isError ? (
              <button
                type="button"
                className="grid size-11 shrink-0 place-items-center rounded-[10px] text-fg-tertiary transition-[background-color,color,transform] duration-110 ease-out hover:bg-surface-hover hover:text-fg active:scale-[0.97] focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-accent"
                onClick={() => dismissToast(t.id)}
                aria-label="Dismiss notification"
              >
                <X className="h-3.5 w-3.5" aria-hidden="true" />
              </button>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
