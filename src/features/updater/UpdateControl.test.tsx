// @vitest-environment happy-dom

import { check, Update } from "@tauri-apps/plugin-updater";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UpdateControl } from "./UpdateControl";

const app = vi.hoisted(() => ({
  showToast: vi.fn(),
  flushPersistence: vi.fn().mockResolvedValue(undefined),
}));
const events = vi.hoisted(() => ({ open: () => {} }));
vi.mock("../../state/app", () => ({ useApp: () => app }));
vi.mock("../../lib/bridge", () => ({
  inTauri: true,
  onToggleRequest: (callback: (event: { open: boolean }) => void) => {
    events.open = () => callback({ open: true });
    return () => {};
  },
}));
vi.mock("@tauri-apps/plugin-updater", async (original) => ({
  ...(await original<typeof import("@tauri-apps/plugin-updater")>()),
  check: vi.fn(),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function update(version: string) {
  const resource = new Update({ rid: 1, version, currentVersion: "0.9.38", rawJson: {} });
  vi.spyOn(resource, "close").mockResolvedValue(undefined);
  vi.spyOn(resource, "download").mockReturnValue(new Promise(() => {}));
  vi.spyOn(resource, "install").mockResolvedValue(undefined);
  return resource;
}

beforeEach(() => {
  vi.clearAllMocks();
  app.flushPersistence.mockResolvedValue(undefined);
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-05T00:00:00Z"));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

async function mountAvailable(resource: Update) {
  vi.mocked(check).mockResolvedValueOnce(resource);
  await act(async () => {
    render(<UpdateControl />);
  });
}

describe("update installation ownership", () => {
  it("flushes edits made during download before starting installation", async () => {
    const resource = update("1.0.0");
    const download = deferred<void>();
    const saving = deferred<void>();
    vi.mocked(resource.download).mockReturnValueOnce(download.promise);
    app.flushPersistence.mockResolvedValueOnce(undefined).mockReturnValueOnce(saving.promise);
    await mountAvailable(resource);
    await act(async () => {
      fireEvent.click(screen.getByRole("button"));
    });
    await act(async () => {
      download.resolve();
    });
    expect(app.flushPersistence).toHaveBeenCalledTimes(2);
    expect(resource.install).not.toHaveBeenCalled();
    await act(async () => {
      saving.resolve();
    });
    expect(resource.install).toHaveBeenCalledOnce();
  });

  it("keeps the installer stopped when the final save fails", async () => {
    const resource = update("1.0.0");
    vi.mocked(resource.download).mockResolvedValueOnce(undefined);
    app.flushPersistence.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("disk full"));
    await mountAvailable(resource);
    await act(async () => {
      fireEvent.click(screen.getByRole("button"));
    });
    expect(resource.install).not.toHaveBeenCalled();
    expect(screen.getByRole("button").textContent).toContain("Retry update");
  });

  it.each(["available", "none", "error"])(
    "ignores a pending check returning %s during installation",
    async (outcome) => {
      const active = update("1.0.0");
      await mountAvailable(active);
      const pending = deferred<Update | null>();
      vi.mocked(check).mockReturnValueOnce(pending.promise);
      await act(async () => {
        vi.setSystemTime(new Date("2026-09-05T00:02:00Z"));
        events.open();
      });
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Install Prism v1.0.0" }));
      });
      const newer = update("1.0.1");
      await act(async () => {
        if (outcome === "error") pending.reject(new Error("offline"));
        else pending.resolve(outcome === "none" ? null : newer);
      });
      expect(active.close).not.toHaveBeenCalled();
      const button = screen.getByRole("button");
      expect(button.getAttribute("aria-busy")).toBe("true");
      expect((button as HTMLButtonElement).disabled).toBe(true);
      fireEvent.click(button);
      expect(active.download).toHaveBeenCalledTimes(1);
      if (outcome === "available") expect(newer.close).toHaveBeenCalledOnce();
    },
  );

  it("saves pending state before allowing the native installer to exit Prism", async () => {
    const resource = update("1.0.0");
    const saving = deferred<void>();
    app.flushPersistence.mockReturnValueOnce(saving.promise);
    await mountAvailable(resource);
    await act(async () => {
      fireEvent.click(screen.getByRole("button"));
    });
    expect(resource.download).not.toHaveBeenCalled();
    expect(screen.getByRole("button").getAttribute("aria-busy")).toBe("true");
    await act(async () => {
      saving.resolve();
    });
    expect(resource.download).toHaveBeenCalledOnce();
  });

  it("does not install if saving fails", async () => {
    const resource = update("1.0.0");
    app.flushPersistence.mockRejectedValueOnce(new Error("disk full"));
    await mountAvailable(resource);
    await act(async () => {
      fireEvent.click(screen.getByRole("button"));
    });
    expect(resource.download).not.toHaveBeenCalled();
    expect(app.showToast).toHaveBeenCalledWith(
      "Settings not saved",
      expect.stringContaining("disk full"),
      "error",
    );
  });

  it("defers closing an active native update resource until installation settles", async () => {
    const resource = update("1.0.0");
    const installation = deferred<void>();
    vi.mocked(resource.download).mockReturnValueOnce(installation.promise);
    await mountAvailable(resource);
    await act(async () => {
      fireEvent.click(screen.getByRole("button"));
    });
    cleanup();
    expect(resource.close).not.toHaveBeenCalled();
    await act(async () => {
      installation.resolve();
    });
    expect(resource.close).toHaveBeenCalledOnce();
  });
});
