#!/usr/bin/env bun
/**
 * Extract the PR body from a Claude Code `--output-format stream-json` run.
 *
 * Claude Code emits newline-delimited JSON (NDJSON) where each line is one
 * event. We take the last `assistant` message's text blocks as the agent's
 * final response (this is what the prompt instructs the agent to format as the
 * PR body). If no assistant text is found, we fall back to the last `result`
 * event's `result` field, then to a placeholder.
 *
 * Usage:
 *   bun scripts/extract-pr-body.ts agent-output.txt > pr-body.md
 *   bun scripts/extract-pr-body.ts - < agent-output.txt > pr-body.md
 */

import { readFileSync } from "node:fs";

function extract(inputPath: string): string {
  const raw = inputPath === "-" ? readFileSync(0, "utf8") : readFileSync(inputPath, "utf8");
  const lines = raw.split("\n");

  let assistantText = "";
  let resultText = "";

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) continue;
    let evt: any;
    try {
      evt = JSON.parse(trimmed);
    } catch {
      continue;
    }

    if (evt?.type === "assistant" && Array.isArray(evt?.message?.content)) {
      const text = evt.message.content
        .filter((b: any) => b?.type === "text" && typeof b.text === "string")
        .map((b: any) => b.text)
        .join("\n");
      if (text.trim() !== "") assistantText = text;
    } else if (evt?.type === "result" && typeof evt?.result === "string") {
      resultText = evt.result;
    }
  }

  const body = assistantText.trim() !== "" ? assistantText : resultText.trim() !== "" ? resultText : "No final message produced by the agent.";
  return body.endsWith("\n") ? body : body + "\n";
}

function main(): void {
  const inputPath = Bun.argv[2] ?? "-";
  process.stdout.write(extract(inputPath));
}

main();
