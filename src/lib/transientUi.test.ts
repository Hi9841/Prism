import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dismissTransientUi, onTransientUiDismiss } from "./transientUi";

describe("transient UI dismissal", () => {
  beforeEach(() => {
    vi.stubGlobal("document", new EventTarget());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("notifies active listeners and removes them cleanly", () => {
    const onDismiss = vi.fn();
    const stopListening = onTransientUiDismiss(onDismiss);

    dismissTransientUi();
    expect(onDismiss).toHaveBeenCalledTimes(1);

    stopListening();
    dismissTransientUi();
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
