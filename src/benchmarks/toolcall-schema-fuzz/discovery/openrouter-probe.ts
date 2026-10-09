import { setTimeout as sleep } from "node:timers/promises";

import type { ToolCall } from "../../../harness/core";
import { Either } from "../../../internal/either";
import { parseSchema, z } from "../../../internal/zod";
import { TOOLCALL_SCHEMA_FUZZ_META } from "../../benchmark-meta";
import { scoreFuzzToolCalls } from "../scorer";
import { TOOLCALL_SCHEMA_FUZZ_SYSTEM_MESSAGE } from "../solver";
import { requestOf } from "../tool-case";
import type { Probe } from "./discover";
import { PASS, TRANSPORT } from "./discover";

const ResponseSchema = z.object({
  choices: z.array(
    z.object({
      message: z.object({
        tool_calls: z
          .array(
            z.object({
              id: z.string(),
              type: z.literal("function"),
              function: z.object({ name: z.string(), arguments: z.string() }),
            })
          )
          .nullish(),
      }),
    })
  ),
});

const REQUEST_REJECTED = "request_rejected";
const REJECTED = new Set([400, 422]);
const RETRYABLE = new Set([408, 429, 500, 502, 503, 504]);

export interface OpenRouterProbeConfig {
  readonly apiKey: string;
  readonly model: string;
  readonly baseUrl: string;
  readonly retries: number;
}

async function send(
  config: OpenRouterProbeConfig,
  body: string
): Promise<{ readonly status: number; readonly text: string }> {
  const response = await fetch(`${config.baseUrl}/chat/completions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${config.apiKey}`,
      "content-type": "application/json",
    },
    body,
  });
  return { status: response.status, text: await response.text() };
}

function toolCallsOf(text: string): readonly ToolCall[] | undefined {
  const json = Either.try((): unknown => JSON.parse(text));
  const parsed = Either.isLeft(json)
    ? json
    : parseSchema(ResponseSchema, json.right);
  return Either.isLeft(parsed)
    ? undefined
    : (parsed.right.choices[0]?.message.tool_calls ?? []);
}

export function openRouterProbe(config: OpenRouterProbeConfig): Probe {
  return async (entry, provider) => {
    const body = JSON.stringify({
      ...requestOf(entry, TOOLCALL_SCHEMA_FUZZ_SYSTEM_MESSAGE),
      model: config.model,
      temperature: TOOLCALL_SCHEMA_FUZZ_META.temperature,
      provider: { only: [provider], allow_fallbacks: false },
    });
    for (let attempt = 0; attempt <= config.retries; attempt += 1) {
      const response = await send(config, body);
      if (RETRYABLE.has(response.status)) {
        await sleep(2 ** attempt * 1000);
        continue;
      }
      if (REJECTED.has(response.status)) {
        return {
          category: REQUEST_REJECTED,
          explanation: `HTTP ${response.status}`,
        };
      }
      const toolCalls =
        response.status === 200 ? toolCallsOf(response.text) : undefined;
      if (toolCalls === undefined) {
        return { category: TRANSPORT, explanation: `HTTP ${response.status}` };
      }
      const score = scoreFuzzToolCalls(entry, toolCalls);
      const [category = score.explanation] = score.explanation.split(":", 1);
      return score.value === "C"
        ? { category: PASS, explanation: score.explanation }
        : { category, explanation: score.explanation };
    }
    return { category: TRANSPORT, explanation: "retries exhausted" };
  };
}
