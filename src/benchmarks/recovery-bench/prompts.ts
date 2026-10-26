import type { ModelMessage } from "../../harness/core";

export const RECOVERY_PREAMBLE =
  "RECOVERY MODE: The previous attempt to complete this task failed. The environment has been restored to the state after the failed attempt. Please analyze what went wrong and try a DIFFERENT approach.";

export const SUMMARIZE_MESSAGES_PROMPT =
  "Please summarize the following conversation concisely, focusing on what was attempted and what went wrong:\n\n";

export const SUMMARY_FALLBACK =
  "Previous attempts to complete this task failed.";

export function buildRecoveryInstruction(
  instruction: string,
  messageContext: string | undefined
): string {
  const parts = [RECOVERY_PREAMBLE];
  if (messageContext !== undefined && messageContext.length > 0) {
    parts.push(`--- PREVIOUS ATTEMPT CONTEXT ---\n${messageContext}`);
  }
  parts.push(`--- ORIGINAL TASK ---\n${instruction}`);
  return parts.join("\n\n");
}

export function formatMessagesAsText(
  messages: readonly ModelMessage[]
): string {
  return messages
    .map((msg) => `[${msg.role.toUpperCase()}]: ${msg.content}`)
    .join("\n\n");
}

export function buildSummarizePrompt(
  messages: readonly ModelMessage[]
): string {
  const plain = messages.map((msg) => ({
    role: msg.role,
    content: msg.content,
  }));
  return `${SUMMARIZE_MESSAGES_PROMPT}${JSON.stringify(plain, null, 2)}`;
}

export function utf8ByteLength(text: string): number {
  return Buffer.byteLength(text, "utf8");
}
