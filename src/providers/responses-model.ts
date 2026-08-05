import { HttpClient, HttpClientRequest } from "@effect/platform";
import type { HttpClientResponse as HttpClientResponseType } from "@effect/platform/HttpClientResponse";
import { Tag } from "effect/Context";
import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  catchTag,
  fail,
  flatMap,
  gen,
  mapError,
  retry,
  succeed,
  sync,
  timeout,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect } from "effect/Layer";
import { runForEach } from "effect/Stream";

import type { ModelUsage } from "../harness/core";
import { ModelError } from "../harness/core";
import type { GenerateConfig } from "../harness/model";
import { stripVariantSuffix } from "../harness/model";
import { Either } from "../internal/either";
import { isRecord } from "../internal/guards";
import { parseSchema, z } from "../internal/zod";
import { recordGenerationId } from "../runtime/generation-ids";
import type { RetryConfig } from "../runtime/retry";
import { rateLimitRetrySchedule } from "../runtime/retry";
import {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
  normalizeBaseUrl,
} from "./openrouter-model";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import {
  appendModelErrorIdentifiers,
  modelErrorIdentifiersFromHeaders,
} from "./request-identifiers";
import { extractMessageText, usageFromResponses } from "./responses-client";

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
): Layer<ResponsesModel, never, HttpClient.HttpClient> {
  const baseUrl = normalizeBaseUrl(
    config.baseUrl ?? "https://openrouter.ai/api/v1"
  );
  return effect(ResponsesModel)(
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      return ResponsesModel.of({
        generate: (input, generateConfig, options) =>
          generate(
            {
              model: config.model,
              input,
              genConfig: generateConfig,
              sessionId: config.sessionId,
              apiKey: config.apiKey,
              baseUrl,
              retry: config.retry,
              onStreamEvent: options?.onStreamEvent,
            },
            client
          ),
      });
    })
  );
}

export interface ResponsesGenerateOpts {
  readonly model: string;
  readonly input: readonly ResponsesInputItem[];
  readonly genConfig: ResponsesGenerateConfig;
  readonly sessionId?: string;
  readonly apiKey: string;
  readonly baseUrl: string;
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
      cost_tier?: GenerateConfig["costTier"];
      cost_quality_tradeoff?: number;
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
    ...(genConfig.costTier !== undefined && { cost_tier: genConfig.costTier }),
    ...(genConfig.costQualityTradeoff !== undefined && {
      cost_quality_tradeoff: genConfig.costQualityTradeoff,
    }),
  };
}

export function generate(
  opts: ResponsesGenerateOpts,
  client: HttpClient.HttpClient
): Effect<ResponsesTurn, ModelError> {
  const { genConfig } = opts;
  const headers: Record<string, string> = {
    Authorization: `Bearer ${opts.apiKey}`,
    "Content-Type": "application/json",
    "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
    "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
  };
  if (genConfig.endpointId !== undefined) {
    headers["X-OR-Endpoint-Id"] = genConfig.endpointId;
  }
  if (genConfig.cloudflareVersion !== undefined) {
    headers["Cloudflare-Workers-Version-Overrides"] =
      genConfig.cloudflareVersion;
  }
  if (opts.sessionId !== undefined) {
    headers["x-session-id"] = opts.sessionId;
  }
  const sendSort =
    genConfig.sort !== undefined && genConfig.endpointId === undefined;
  const hasTimeout =
    genConfig.timeoutMs !== undefined && genConfig.timeoutMs > 0;
  const baseModel = stripVariantSuffix(opts.model);
  const isAutoRouter =
    baseModel === "openrouter/auto" || baseModel === "openrouter/auto-beta";
  const autoRouterPlugin = buildAutoRouterPlugin(
    baseModel,
    genConfig,
    isAutoRouter
  );
  return gen(function* () {
    const startedAt = performance.now();
    let identifiers: ModelErrorIdentifiers = {};
    const body = {
      model: opts.model,
      input: [...opts.input],
      stream: true,
      store: false,
      cache_control: { type: "ephemeral" },
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
        max_output_tokens: genConfig.maxTokens,
      }),
      ...(sendSort && { provider: { sort: genConfig.sort } }),
      ...(autoRouterPlugin !== undefined && { plugins: [autoRouterPlugin] }),
      ...genConfig.extraBody,
    };
    const request = HttpClientRequest.post(`${opts.baseUrl}/responses`).pipe(
      HttpClientRequest.setHeaders(headers),
      HttpClientRequest.bodyUnsafeJson(body)
    );
    const requestAttempt = gen(function* () {
      identifiers = {};
      const response = yield* client.execute(request);
      identifiers = modelErrorIdentifiersFromHeaders(response.headers);
      if (response.status < 200 || response.status >= 300) {
        const text = yield* response.text;
        return yield* fail(
          new ModelError({
            status: response.status,
            message: appendModelErrorIdentifiers(
              `OpenRouter HTTP ${response.status}: ${text}`,
              identifiers
            ),
            ...identifiers,
            ...(response.status === 429 && {
              retryAfterMs: parseRetryAfter(
                response.headers["retry-after"] ?? null
              ),
            }),
          })
        );
      }
      const terminalResponse = yield* consumeSse(
        response,
        identifiers,
        opts.onStreamEvent
      );
      return { json: terminalResponse, startedAt, identifiers };
    });
    return yield* hasTimeout
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
  }).pipe(
    mapError(toModelError),
    retry(rateLimitRetrySchedule(opts.retry ?? {})),
    flatMap(({ json, startedAt, identifiers }) =>
      decodeResult(json, startedAt, identifiers)
    )
  );
}

function consumeSse(
  response: HttpClientResponseType,
  initialIdentifiers: ModelErrorIdentifiers,
  onStreamEvent?: (event: Record<string, unknown>) => void
): Effect<unknown, ModelError> {
  return gen(function* () {
    const decoder = new TextDecoder();
    let buffer = "";
    let terminalResponse: unknown;
    let streamError: ModelError | undefined;
    const identifiers = initialIdentifiers;
    const parseFrame = (frame: string): void => {
      const data = frame
        .split(/\r?\n/)
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trimStart())
        .join("\n");
      if (data.length === 0 || data === "[DONE]") {
        return;
      }
      const parsed = parseJson(data);
      if (!isRecord(parsed)) {
        streamError = new ModelError({
          status: 500,
          message: appendModelErrorIdentifiers(
            "OpenRouter stream emitted invalid JSON",
            identifiers
          ),
          ...identifiers,
        });
        return;
      }
      const eventResponse = parsed["response"];
      const eventId =
        isRecord(eventResponse) && typeof eventResponse["id"] === "string"
          ? eventResponse["id"]
          : undefined;
      if (eventId !== undefined) {
        Object.assign(identifiers, { generationId: eventId });
      }
      onStreamEvent?.(parsed);
      const type = parsed["type"];
      if (type === "response.failed" || type === "error") {
        streamError = new ModelError({
          status: 500,
          message: appendModelErrorIdentifiers(
            `OpenRouter stream error: ${streamErrorMessage(parsed)}`,
            identifiers
          ),
          ...identifiers,
        });
        return;
      }
      if (type === "response.completed" || type === "response.incomplete") {
        terminalResponse = parsed["response"];
      }
    };
    yield* response.stream.pipe(
      runForEach((chunk) =>
        sync(() => {
          buffer += decoder.decode(chunk, { stream: true });
          const frames = buffer.split(/\r?\n\r?\n/);
          buffer = frames.pop() ?? "";
          for (const frame of frames) {
            parseFrame(frame);
          }
        })
      ),
      mapError(
        (cause) =>
          new ModelError({
            status: 500,
            message: appendModelErrorIdentifiers(
              `OpenRouter stream error: ${String(cause)}`,
              identifiers
            ),
            ...identifiers,
          })
      )
    );
    buffer += decoder.decode();
    if (buffer.length > 0) {
      parseFrame(buffer);
    }
    if (streamError !== undefined) {
      return yield* fail(streamError);
    }
    if (terminalResponse === undefined) {
      return yield* fail(
        new ModelError({
          status: 500,
          message: appendModelErrorIdentifiers(
            "OpenRouter stream ended without a terminal response",
            identifiers
          ),
          ...identifiers,
        })
      );
    }
    return terminalResponse;
  }).pipe(
    mapError((cause) =>
      cause instanceof ModelError
        ? cause
        : new ModelError({
            status: 500,
            message: appendModelErrorIdentifiers(
              `OpenRouter stream error: ${String(cause)}`,
              initialIdentifiers
            ),
            ...initialIdentifiers,
          })
    )
  );
}

function streamErrorMessage(event: Record<string, unknown>): string {
  const response = event["response"];
  if (
    isRecord(response) &&
    isRecord(response["error"]) &&
    typeof response["error"]["message"] === "string"
  ) {
    return response["error"]["message"];
  }
  return typeof event["message"] === "string"
    ? event["message"]
    : String(response ?? event);
}

function parseJson(data: string): unknown {
  try {
    return JSON.parse(data);
  } catch {
    return null;
  }
}

const ResponsesResultSchema = z.object({
  id: z.string().nullish(),
  output: z.array(z.record(z.string(), z.unknown())),
  usage: z.record(z.string(), z.unknown()).nullish(),
});

function decodeResult(
  raw: unknown,
  startedAt: number,
  identifiers: ModelErrorIdentifiers
): Effect<ResponsesTurn, ModelError> {
  const parseResult = parseSchema(ResponsesResultSchema, raw);
  if (Either.isLeft(parseResult)) {
    return fail(
      new ModelError({
        message: appendModelErrorIdentifiers(
          `OpenRouter response failed validation: ${parseResult.left.message}`,
          identifiers
        ),
        ...identifiers,
      })
    );
  }
  const outputItems = parseResult.right.output;
  const usage = usageFromResponses(parseResult.right.usage ?? null);
  const functionCalls = outputItems.flatMap((item) => {
    if (
      item["type"] !== "function_call" ||
      typeof item["call_id"] !== "string" ||
      typeof item["name"] !== "string" ||
      typeof item["arguments"] !== "string"
    ) {
      return [];
    }
    return [
      {
        callId: item["call_id"],
        name: item["name"],
        arguments: item["arguments"],
      },
    ];
  });
  return recordGenerationId(parseResult.right.id).pipe(
    flatMap(() =>
      succeed({
        outputItems,
        functionCalls,
        text: extractMessageText(outputItems),
        ...(usage !== undefined && { usage }),
        generationTimeMs: Math.round(performance.now() - startedAt),
      })
    )
  );
}

function toModelError(cause: unknown): ModelError {
  if (cause instanceof ModelError) {
    return cause;
  }
  return new ModelError({
    message: `OpenRouter request failed: ${String(cause)}`,
  });
}

function parseRetryAfter(value: string | null): number | undefined {
  if (value === null) {
    return undefined;
  }
  const seconds = Number(value);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds * 1e3 : undefined;
}
