import type { ResponsesRequest, StreamEvents } from "@openrouter/sdk/models";
import { Tag } from "effect/Context";
import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  catchTag,
  fail,
  gen,
  map,
  mapError,
  retry,
  suspend,
  timeout,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";

import type { ModelUsage } from "../harness/core";
import { ModelError } from "../harness/core";
import type { GenerateConfig } from "../harness/model";
import { stripVariantSuffix } from "../harness/model";
import { isRecord } from "../internal/guards";
import type { RetryConfig } from "../runtime/retry";
import { rateLimitRetrySchedule } from "../runtime/retry";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import { appendModelErrorIdentifiers } from "./request-identifiers";
import type { ResponsesResult, ResponsesService } from "./responses-client";
import {
  extractMessageText,
  makeResponsesLayer,
  Responses,
  toModelError,
  usageFromResponses,
} from "./responses-client";

export type ResponsesInputItem = Record<string, unknown>;

export interface ResponsesFunctionTool {
  readonly type: "function";
  readonly name: string;
  readonly description?: string;
  readonly parameters: Record<string, unknown>;
}

export interface ResponsesGenerateConfig extends Omit<GenerateConfig, "tools"> {
  readonly instructions?: string;
  readonly tools?: readonly ResponsesFunctionTool[];
}

export interface ResponsesTurn {
  readonly outputItems: Record<string, unknown>[];
  readonly functionCalls: readonly {
    readonly callId: string;
    readonly name: string;
    readonly arguments: string;
  }[];
  readonly text: string;
  readonly usage?: ModelUsage;
  readonly generationTimeMs: number;
}

export interface ResponsesModelConfig {
  readonly model: string;
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly retry?: RetryConfig;
}

export interface ResponsesModelService {
  readonly generate: (
    input: readonly ResponsesInputItem[],
    config: ResponsesGenerateConfig,
    options?: {
      readonly onStreamEvent?: (event: Record<string, unknown>) => void;
    }
  ) => Effect<ResponsesTurn, ModelError>;
}

export class ResponsesModel extends Tag(
  "@openrouter/bench-harness/responses-model/ResponsesModel"
)<ResponsesModel, ResponsesModelService>() {}

export function makeResponsesModelLayer(
  config: ResponsesModelConfig
): Layer<ResponsesModel> {
  const responsesLayer = makeResponsesLayer({
    apiKey: config.apiKey,
    ...(config.baseUrl !== undefined && { baseUrl: config.baseUrl }),
    ...(config.sessionId !== undefined && { sessionId: config.sessionId }),
  });
  return effect(ResponsesModel)(
    gen(function* () {
      const responses = yield* Responses;
      return ResponsesModel.of({
        generate: (input, generateConfig, options) =>
          generate(
            {
              model: config.model,
              input,
              genConfig: generateConfig,
              retry: config.retry,
              onStreamEvent: options?.onStreamEvent,
            },
            responses
          ),
      });
    })
  ).pipe(provide(responsesLayer));
}

export interface ResponsesGenerateOpts {
  readonly model: string;
  readonly input: readonly ResponsesInputItem[];
  readonly genConfig: ResponsesGenerateConfig;
  readonly retry?: RetryConfig;
  readonly onStreamEvent?: (event: Record<string, unknown>) => void;
}

function buildAutoRouterPlugin(
  baseModel: string,
  genConfig: ResponsesGenerateConfig,
  isAutoRouter: boolean
):
  | {
      id: "auto-router" | "auto-beta-router";
      costTier?: GenerateConfig["costTier"];
      costQualityTradeoff?: number;
    }
  | undefined {
  if (
    !isAutoRouter ||
    (genConfig.costTier === undefined &&
      genConfig.costQualityTradeoff === undefined)
  ) {
    return undefined;
  }
  return {
    id:
      baseModel === "openrouter/auto-beta" ? "auto-beta-router" : "auto-router",
    ...(genConfig.costTier !== undefined && { costTier: genConfig.costTier }),
    ...(genConfig.costQualityTradeoff !== undefined && {
      costQualityTradeoff: genConfig.costQualityTradeoff,
    }),
  };
}

export function generate(
  opts: ResponsesGenerateOpts,
  responses: ResponsesService
): Effect<ResponsesTurn, ModelError> {
  const { genConfig } = opts;
  const sendSort =
    genConfig.sort !== undefined && genConfig.endpointId === undefined;
  const baseModel = stripVariantSuffix(opts.model);
  const isAutoRouter =
    baseModel === "openrouter/auto" || baseModel === "openrouter/auto-beta";
  const autoRouterPlugin = buildAutoRouterPlugin(
    baseModel,
    genConfig,
    isAutoRouter
  );
  const body = {
    model: opts.model,
    store: false,
    include: ["reasoning.encrypted_content"],
    ...(genConfig.instructions !== undefined && {
      instructions: genConfig.instructions,
    }),
    ...(genConfig.tools !== undefined &&
      genConfig.tools.length > 0 && { tools: [...genConfig.tools] }),
    ...(genConfig.reasoningEffort !== undefined && {
      reasoning: { effort: genConfig.reasoningEffort },
    }),
    ...(genConfig.temperature !== undefined && {
      temperature: genConfig.temperature,
    }),
    ...(genConfig.maxTokens !== undefined && {
      maxOutputTokens: genConfig.maxTokens,
    }),
    ...(sendSort && { provider: { sort: genConfig.sort } }),
    ...(autoRouterPlugin !== undefined && { plugins: [autoRouterPlugin] }),
  } satisfies ResponsesRequest;
  const extraHeaders = {
    ...(genConfig.endpointId !== undefined && {
      "X-OR-Endpoint-Id": genConfig.endpointId,
    }),
    ...(genConfig.cloudflareVersion !== undefined && {
      "Cloudflare-Workers-Version-Overrides": genConfig.cloudflareVersion,
    }),
  };
  const extraBody = {
    input: [...opts.input],
    ...genConfig.extraBody,
  };
  let identifiers: ModelErrorIdentifiers = {};
  const requestAttempt = suspend(() => {
    identifiers = {};
    const startedAt = performance.now();
    return responses
      .send(body, {
        ...(Object.keys(extraHeaders).length > 0 && { extraHeaders }),
        extraBody,
        onResponseIdentifiers: (responseIdentifiers) => {
          identifiers = responseIdentifiers;
        },
        onStreamEvent: (event: StreamEvents) => {
          const eventValue: unknown = event;
          const rawEvent =
            isRecord(eventValue) && isRecord(eventValue["raw"])
              ? eventValue["raw"]
              : eventValue;
          if (isRecord(rawEvent)) {
            const response = rawEvent["response"];
            if (isRecord(response) && typeof response["id"] === "string") {
              identifiers = { ...identifiers, generationId: response["id"] };
            }
            opts.onStreamEvent?.(rawEvent);
          }
        },
      })
      .pipe(
        mapError(toModelError),
        map((result) =>
          toResponsesTurn(result, Math.round(performance.now() - startedAt))
        )
      );
  });
  const hasTimeout =
    genConfig.timeoutMs !== undefined && genConfig.timeoutMs > 0;
  const timedAttempt = hasTimeout
    ? requestAttempt.pipe(
        timeout(millis(genConfig.timeoutMs!)),
        catchTag("TimeoutException", () =>
          fail(
            new ModelError({
              status: 408,
              message: appendModelErrorIdentifiers(
                `Request timed out after ${genConfig.timeoutMs}ms`,
                identifiers
              ),
              ...identifiers,
            })
          )
        )
      )
    : requestAttempt;
  return timedAttempt.pipe(retry(rateLimitRetrySchedule(opts.retry ?? {})));
}

function toLegacyOutputItem(
  item: Readonly<Record<string, unknown>>
): Record<string, unknown> {
  const { callId, encryptedContent, ...legacyItem } = item;
  return {
    ...legacyItem,
    ...(typeof callId === "string" && { call_id: callId }),
    ...(typeof encryptedContent === "string" && {
      encrypted_content: encryptedContent,
    }),
  };
}

function toResponsesTurn(
  result: ResponsesResult,
  generationTimeMs: number
): ResponsesTurn {
  const outputItems = result.output.map(toLegacyOutputItem);
  const usage = usageFromResponses(result.usage);
  const functionCalls = outputItems.flatMap((item) => {
    const callId = item["callId"] ?? item["call_id"];
    if (
      item["type"] !== "function_call" ||
      typeof callId !== "string" ||
      typeof item["name"] !== "string" ||
      typeof item["arguments"] !== "string"
    ) {
      return [];
    }
    return [
      {
        callId,
        name: item["name"],
        arguments: item["arguments"],
      },
    ];
  });
  return {
    outputItems,
    functionCalls,
    text: extractMessageText(outputItems),
    ...(usage !== undefined && { usage }),
    generationTimeMs,
  };
}
