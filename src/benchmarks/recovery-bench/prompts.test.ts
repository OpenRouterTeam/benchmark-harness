import { describe, expect, it } from "bun:test";

import { MessageRole } from "../../harness/core";
import {
  buildRecoveryInstruction,
  buildSummarizePrompt,
  formatMessagesAsText,
  RECOVERY_PREAMBLE,
  SUMMARIZE_MESSAGES_PROMPT,
  utf8ByteLength,
} from "./prompts";

const MESSAGES = [
  { role: MessageRole.User, content: "fix the build" },
  { role: MessageRole.Assistant, content: "running make" },
] as const;

describe("recovery-bench prompts", () => {
  it("builds the upstream recovery instruction with prior context", () => {
    const text = buildRecoveryInstruction("Task body", "what happened");
    expect(text).toBe(
      `${RECOVERY_PREAMBLE}\n\n--- PREVIOUS ATTEMPT CONTEXT ---\nwhat happened\n\n--- ORIGINAL TASK ---\nTask body`
    );
  });

  it("omits the context block when there is no context", () => {
    expect(buildRecoveryInstruction("Task body", undefined)).toBe(
      `${RECOVERY_PREAMBLE}\n\n--- ORIGINAL TASK ---\nTask body`
    );
    expect(buildRecoveryInstruction("Task body", "")).toBe(
      `${RECOVERY_PREAMBLE}\n\n--- ORIGINAL TASK ---\nTask body`
    );
  });

  it("formats prior messages with upper-cased role tags", () => {
    expect(formatMessagesAsText(MESSAGES)).toBe(
      "[USER]: fix the build\n\n[ASSISTANT]: running make"
    );
  });

  it("builds the summarization prompt as JSON after the upstream preamble", () => {
    const prompt = buildSummarizePrompt(MESSAGES);
    expect(prompt.startsWith(SUMMARIZE_MESSAGES_PROMPT)).toBe(true);
    const json: unknown = JSON.parse(
      prompt.slice(SUMMARIZE_MESSAGES_PROMPT.length)
    );
    expect(json).toEqual([
      { role: "user", content: "fix the build" },
      { role: "assistant", content: "running make" },
    ]);
  });

  it("measures UTF-8 bytes rather than code units", () => {
    expect(utf8ByteLength("abc")).toBe(3);
    expect(utf8ByteLength("é")).toBe(2);
  });
});
