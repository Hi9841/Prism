import { describe, expect, it } from "vitest";
import { dedupeApps, fuzzy, fuzzyApps, fuzzyScore } from "./search";
import type { AppEntry } from "./types";

describe("fuzzyScore", () => {
  it("matches prefixes strongly", () => {
    const cal = fuzzyScore("cal", "Calendar");
    const coal = fuzzyScore("cal", "Coalesce");
    expect(cal).not.toBeNull();
    expect(coal).not.toBeNull();
    expect(cal as number).toBeGreaterThan(coal as number);
  });

  it("matches subsequences like a launcher", () => {
    expect(fuzzyScore("clndr", "Calendar")).not.toBeNull();
    expect(fuzzyScore("vsc", "Visual Studio Code")).not.toBeNull();
    expect(fuzzyScore("gogl", "Google")).not.toBeNull();
  });

  it("prefers word-start and camel matches", () => {
    const a = fuzzyScore("st", "Studio");
    const b = fuzzyScore("st", "Fast");
    expect(a).not.toBeNull();
    expect(b).not.toBeNull();
    expect(a as number).toBeGreaterThan(b as number);
  });

  it("rejects non-subsequences", () => {
    expect(fuzzyScore("xyz", "abc")).toBeNull();
    expect(fuzzyScore("", "abc")).toBeNull();
    expect(fuzzyScore("abcef", "abc")).toBeNull();
  });
});

describe("fuzzy", () => {
  const pool = [
    { id: "a", title: "Calculator" },
    { id: "b", title: "Calendar" },
    { id: "c", title: "Clock" },
    { id: "d", title: "Chrome" },
    { id: "e", title: "Terminal" },
  ];

  it("ranks prefix matches first", () => {
    const hits = fuzzy(pool, "cal", { limit: 5 });
    expect(hits[0].item.id).toBe("b");
    expect(hits[1].item.id).toBe("a");
  });

  it("finds fuzzy matches", () => {
    const hits = fuzzy(pool, "clndr", { limit: 5 });
    expect(hits[0].item.id).toBe("b");
  });

  it("returns nothing for empty query", () => {
    expect(fuzzy(pool, "", { limit: 5 })).toEqual([]);
  });

  it("respects the limit", () => {
    expect(fuzzy(pool, "c", { limit: 2 })).toHaveLength(2);
  });

  it("scores keywords below titles", () => {
    const withKws = [
      { id: "x", title: "Prism" },
      { id: "y", title: "Something Else" },
    ];
    const hits = fuzzy(
      withKws.map((i) => ({ ...i, keywords: i.id === "x" ? ["shortcut", "palette"] : [] })),
      "shortcut",
      { limit: 2 },
    );
    expect(hits[0].item.id).toBe("x");
  });
});

describe("fuzzyApps", () => {
  const apps: AppEntry[] = [
    {
      name: "Fortnite",
      appId: "fortnite",
      normalizedName: "fortnite",
      keywords: ["fortniteclient-win64-shipping", "documents"],
    },
    { name: "Fortress", appId: "fortress", normalizedName: "fortress" },
    { name: "Visual Studio Code", appId: "vsc", normalizedName: "visualstudiocode", keywords: ["code.exe"] },
    { name: "Steam", appId: "steam", normalizedName: "steam" },
    { name: "Discord", appId: "discord", normalizedName: "discord" },
  ];

  it("ranks exact and prefix name matches first", () => {
    const hits = fuzzyApps(apps, "fort", 8);
    expect(hits[0].name).toBe("Fortnite");
    expect(hits[1].name).toBe("Fortress");
  });

  it("is case-insensitive", () => {
    expect(fuzzyApps(apps, "FORTNITE", 8)[0].name).toBe("Fortnite");
  });

  it("matches the exe name via keywords (partial word)", () => {
    const hits = fuzzyApps(apps, "shipping", 8);
    expect(hits.map((h) => h.name)).toContain("Fortnite");
  });

  it("matches each word of a multi-word query", () => {
    const hits = fuzzyApps(apps, "studio code", 8);
    expect(hits[0].name).toBe("Visual Studio Code");
  });

  it("matches punctuation-free normalized names", () => {
    const hits = fuzzyApps(apps, "visualstudiocode", 8);
    expect(hits[0].name).toBe("Visual Studio Code");
  });

  it("prefix beats fuzzy and keyword matches", () => {
    const hits = fuzzyApps(apps, "ste", 8);
    expect(hits[0].name).toBe("Steam");
  });

  it("returns nothing for empty query", () => {
    expect(fuzzyApps(apps, "", 8)).toEqual([]);
  });

  it("respects the limit", () => {
    expect(fuzzyApps(apps, "e", 2)).toHaveLength(2);
  });

  it("collapses Windows aliases and rejects distant fuzzy noise", () => {
    const aliases: AppEntry[] = [
      {
        name: "WezTerm",
        appId: "wezterm-shortcut",
        normalizedName: "wezterm",
        source: "startMenu",
      },
      {
        name: "WezTerm",
        appId: "org.wezfurlong.wezterm",
        normalizedName: "wezterm",
        source: "appsFolder",
      },
      {
        name: "wezterm",
        appId: "wezterm-cli",
        normalizedName: "wezterm",
        source: "programs",
      },
      {
        name: "Windows Performance Analyzer",
        appId: "wpa",
        normalizedName: "windowsperformanceanalyzer",
        source: "startMenu",
      },
    ];

    const hits = fuzzyApps(aliases, "wez", 8);
    expect(hits).toHaveLength(1);
    expect(hits[0].appId).toBe("wezterm-shortcut");
  });

  it("deduplicates the unfiltered app list with the best launch source", () => {
    const aliases: AppEntry[] = [
      { name: "Prism", appId: "aumid", normalizedName: "prism", source: "appsFolder" },
      { name: "Prism", appId: "shortcut", normalizedName: "prism", source: "startMenu" },
    ];
    expect(dedupeApps(aliases).map((app) => app.appId)).toEqual(["shortcut"]);
  });
});
