import { describe, expect, it } from "vitest";
import { sanitizeHistory } from "./app";

describe("sanitizeHistory", () => {
  it("keeps valid unique entries and rejects malformed state", () => {
    expect(
      sanitizeHistory([
        { id: "file::d::C:\\Docs", title: "Docs", ts: 2 },
        { id: "file::d::C:\\Docs", title: "Duplicate", ts: 1 },
        { id: 42, title: "Invalid", ts: 0 },
        null,
      ]),
    ).toEqual([{ id: "file::d::C:\\Docs", title: "Docs", ts: 2 }]);
  });

  it("caps persisted history", () => {
    const entries = Array.from({ length: 30 }, (_, index) => ({
      id: `app::${index}`,
      title: `App ${index}`,
      ts: index,
    }));
    expect(sanitizeHistory(entries)).toHaveLength(20);
  });
});
