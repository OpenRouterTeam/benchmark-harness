import type { ReasoningDetails } from "../harness/reasoning-details";
import { definedValues, isRecord } from "../internal/guards";

export interface ResponsesReasoning {
  readonly reasoning?: string;
  readonly reasoningDetails?: ReasoningDetails;
}

const ReasoningDetailType = {
  Summary: "reasoning.summary",
  Encrypted: "reasoning.encrypted",
  Text: "reasoning.text",
} as const;

const REASONING_JOINER = "\n\n";

export function reasoningFromOutputItems(
  outputItems: readonly Record<string, unknown>[]
): ResponsesReasoning {
  const details: unknown[] = [];
  const texts: string[] = [];
  const summaries: string[] = [];

  for (const item of outputItems) {
    if (item["type"] !== "reasoning") {
      continue;
    }
    const id = stringField(item, "id");
    const format = stringField(item, "format");
    const signature = stringField(item, "signature");

    for (const text of reasoningTexts(item)) {
      texts.push(text);
      details.push(
        definedValues({
          type: ReasoningDetailType.Text,
          text,
          id,
          format,
          signature,
        })
      );
    }

    for (const summary of summaryTexts(item)) {
      summaries.push(summary);
      details.push(
        definedValues({
          type: ReasoningDetailType.Summary,
          summary,
          id,
          format,
        })
      );
    }

    const encrypted = stringField(item, "encrypted_content");
    if (encrypted !== undefined) {
      details.push(
        definedValues({
          type: ReasoningDetailType.Encrypted,
          data: encrypted,
          id,
          format,
        })
      );
    }
  }

  const readable = texts.length > 0 ? texts : summaries;
  return definedValues({
    reasoning:
      readable.length > 0 ? readable.join(REASONING_JOINER) : undefined,
    reasoningDetails: details.length > 0 ? details : undefined,
  });
}

function reasoningTexts(item: Record<string, unknown>): string[] {
  return partTexts(item["content"], "text");
}

function summaryTexts(item: Record<string, unknown>): string[] {
  return partTexts(item["summary"], "text");
}

function partTexts(parts: unknown, key: string): string[] {
  if (!Array.isArray(parts)) {
    return [];
  }
  return parts.flatMap((part) => {
    if (!isRecord(part)) {
      return [];
    }
    const text = stringField(part, key);
    return text !== undefined ? [text] : [];
  });
}

function stringField(
  record: Record<string, unknown>,
  key: string
): string | undefined {
  const value = record[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
