import { HttpClient, HttpClientRequest } from "@effect/platform";
import type { ChatUsage } from "@openrouter/sdk/models";
import { ChatResult$inboundSchema } from "@openrouter/sdk/models/chatresult";
import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  fail,
  flatMap,
  gen,
  mapError,
  retry,
  succeed,
  sync,
  tapError,
  timeout,
  catchTag,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect } from "effect/Layer";

import type {
  ChatMessage,
  ContentPart,
  ModelOutput,
  ModelUsage,
} from "../harness/core";
import { MessageRole, ModelError } from "../harness/core";
import type { GenerateConfig } from "../harness/model";
import { Model, stripVariantSuffix } from "../harness/model";
import type { ReasoningDetails } from "../harness/reasoning-details";
import { hasReasoningDetails } from "../harness/reasoning-details";
import { Either } from "../internal/either";
import { unknownErrorToString } from "../internal/errors";
import { isDefinedAndNotNull, isRecord } from "../internal/guards";
import { wLog } from "../internal/log";
import { parseSchema, z } from "../internal/zod";
import { recordGenerationId } from "../runtime/generation-ids";
import {
  buildResponseCacheSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  RESPONSE_CACHE_HEADER,
  RESPONSE_CACHE_SALT_FIELD,
  withRetryAttemptSalt,
} from "../runtime/response-cache";
import type { RetryConfig } from "../runtime/retry";
import { rateLimitRetrySchedule } from "../runtime/retry";
import {
  buildAutoRouterPlugin,
  toWireAutoRouterPlugin,
} from "./auto-router-plugin";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import {
  appendModelErrorIdentifiers,
  modelErrorIdentifiersFromHeaders,
} from "./request-identifiers";

export const BENCH_HARNESS_APP_REFERRER =
  "https://bench-harness.openrouter.ai/";

export const BENCH_HARNESS_APP_TITLE = "OpenRouter: Bench Harness";

export interface OpenRouterModelConfig {
  readonly model: string;
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly retry?: RetryConfig;
}

export function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.replace(/\/+$/u, "");
  return trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
}

export function makeOpenRouterModelLayer(
  config: OpenRouterModelConfig
): Layer<Model, never, HttpClient.HttpClient> {
  const baseUrl = normalizeBaseUrl(
    config.baseUrl ?? "https://openrouter.ai/api/v1"
  );
  return effect(Model)(
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      return Model.of({
        generate: (messages, genConfig) =>
          generate(
            {
              model: config.model,
              messages,
              genConfig,
              sessionId: config.sessionId,
              apiKey: config.apiKey,
              baseUrl,
              retry: config.retry,
            },
            client
          ),
      });
    })
  );
}

interface GenerateOpts {
  readonly model: string;
  readonly messages: readonly ChatMessage[];
  readonly genConfig: GenerateConfig;
  readonly sessionId?: string;
  readonly apiKey: string;
  readonly baseUrl: string;
  readonly retry?: RetryConfig;
}

export function generate(
  opts: GenerateOpts,
  client: HttpClient.HttpClient
): Effect<ModelOutput, ModelError> {
  const { model, messages, genConfig } = opts;
  const headers: Record<string, string> = {
    Authorization: `Bearer ${opts.apiKey}`,
    "Content-Type": "application/json",
    "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
    "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
    [RESPONSE_CACHE_HEADER]: "true",
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
  const baseModel = stripVariantSuffix(model);
  const autoRouterPlugin = buildAutoRouterPlugin(baseModel, genConfig);
  const wireAutoRouterPlugin =
    autoRouterPlugin === undefined
      ? undefined
      : toWireAutoRouterPlugin(autoRouterPlugin);
  const attempt = gen(function* () {
    const startedAt = performance.now();
    const epoch = yield* getCurrentEpoch;
    const retryAttempt = yield* getCurrentRetryAttempt;
    const cacheSalt = buildResponseCacheSalt(
      opts.sessionId,
      epoch,
      retryAttempt
    );
    const body = {
      model,
      messages: messages.map(toApiMessage),
      stream: false,
      cache_control: { type: "ephemeral" },
      ...(genConfig.temperature !== undefined && {
        temperature: genConfig.temperature,
      }),
      ...(genConfig.maxTokens !== undefined && {
        max_tokens: genConfig.maxTokens,
      }),
      ...(genConfig.tools !== undefined &&
        genConfig.tools.length > 0 && { tools: [...genConfig.tools] }),
      ...(genConfig.reasoningEffort !== undefined && {
        reasoning_effort: genConfig.reasoningEffort,
      }),
      ...(sendSort && { provider: { sort: genConfig.sort } }),
      ...(wireAutoRouterPlugin !== undefined && {
        plugins: [wireAutoRouterPlugin],
      }),
      ...(cacheSalt !== undefined && {
        [RESPONSE_CACHE_SALT_FIELD]: cacheSalt,
      }),
      ...genConfig.extraBody,
    };
    const request = HttpClientRequest.post(
      `${opts.baseUrl}/chat/completions`
    ).pipe(
      HttpClientRequest.setHeaders(headers),
      HttpClientRequest.bodyUnsafeJson(body)
    );
    const response = yield* hasTimeout
      ? client.execute(request).pipe(
          timeout(millis(genConfig.timeoutMs!)),
          catchTag("TimeoutException", () =>
            fail(
              new ModelError({
                status: 408,
                message: `Request timed out after ${genConfig.timeoutMs}ms`,
              })
            )
          )
        )
      : client.execute(request);
    const identifiers = modelErrorIdentifiersFromHeaders(response.headers);
    const retryAfterHeader = response.headers["retry-after"] ?? null;
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
            retryAfterMs: parseRetryAfter(retryAfterHeader),
          }),
        })
      );
    }
    const rawBody = yield* response.text;
    const json = yield* parseJsonBody(rawBody, identifiers);
    const envelopeError = errorEnvelopeError(
      json,
      identifiers,
      retryAfterHeader
    );
    if (envelopeError) {
      logUnusableBody(rawBody, envelopeError, identifiers);
      return yield* fail(envelopeError);
    }
    return yield* decodeResult(json, startedAt, identifiers).pipe(
      tapError((error) =>
        sync(() => {
          logUnusableBody(rawBody, error, identifiers);
        })
      )
    );
  });
  return withRetryAttemptSalt(attempt).pipe(
    mapError(toModelError),
    retry(rateLimitRetrySchedule(opts.retry ?? {}))
  );
}

type ResponseIdentifiers = Pick<ModelErrorIdentifiers, "cfRay" | "xRequestId">;

const RAW_BODY_LOG_LIMIT = 2000;

const errorEnvelopeSchema = z.object({
  choices: z.array(z.unknown()).nullish(),
  error: z.object({
    message: z.string().optional(),
    code: z.union([z.number(), z.string()]).optional(),
  }),
});

function parseJsonBody(
  rawBody: string,
  identifiers: ResponseIdentifiers
): Effect<unknown, ModelError> {
  try {
    const parsed: unknown = JSON.parse(rawBody);
    return succeed(parsed);
  } catch (cause) {
    const error = new ModelError({
      message: appendModelErrorIdentifiers(
        `OpenRouter 2xx body was not JSON: ${unknownErrorToString(cause)}`,
        identifiers
      ),
      ...identifiers,
    });
    logUnusableBody(rawBody, error, identifiers);
    return fail(error);
  }
}

function errorEnvelopeError(
  json: unknown,
  identifiers: ResponseIdentifiers,
  retryAfterHeader: string | null
): ModelError | undefined {
  const parsed = parseSchema(errorEnvelopeSchema, json);
  if (Either.isLeft(parsed)) {
    return undefined;
  }
  const { choices } = parsed.right;
  if (isDefinedAndNotNull(choices) && choices.length > 0) {
    return undefined;
  }
  const { code, message } = parsed.right.error;
  const status = toStatus(code);
  const details = [
    code === undefined ? undefined : `code ${code}`,
    message,
  ].filter((detail): detail is string => detail !== undefined);
  return new ModelError({
    ...(status !== undefined && { status }),
    message: appendModelErrorIdentifiers(
      details.length > 0
        ? `OpenRouter HTTP 200 error envelope (${details.join(": ")})`
        : "OpenRouter HTTP 200 error envelope",
      identifiers
    ),
    ...identifiers,
    ...(status === 429 && { retryAfterMs: parseRetryAfter(retryAfterHeader) }),
  });
}

function toStatus(code: number | string | undefined): number | undefined {
  if (code === undefined) {
    return undefined;
  }
  const parsed = typeof code === "number" ? code : Number(code);
  const isHttpStatus =
    Number.isInteger(parsed) && parsed >= 100 && parsed <= 599;
  return isHttpStatus ? parsed : undefined;
}

function logUnusableBody(
  rawBody: string,
  error: ModelError,
  identifiers: ResponseIdentifiers
): void {
  wLog("OpenRouter 2xx response did not yield a completion", {
    error_message: error.message,
    ...(error.status !== undefined && { error_status: error.status }),
    raw_body:
      rawBody.length > RAW_BODY_LOG_LIMIT
        ? `${rawBody.slice(0, RAW_BODY_LOG_LIMIT - 3)}...`
        : rawBody,
    ...(identifiers.cfRay !== undefined && { cf_ray: identifiers.cfRay }),
    ...(identifiers.xRequestId !== undefined && {
      x_request_id: identifiers.xRequestId,
    }),
  });
}

function toApiContentItem(part: ContentPart) {
  switch (part.type) {
    case "text": {
      return { type: "text", text: part.text };
    }
    case "image_url": {
      return {
        type: "image_url",
        image_url: {
          url: part.imageUrl.url,
          ...(part.imageUrl.detail !== undefined && {
            detail: part.imageUrl.detail,
          }),
        },
      };
    }
    default: {
      return part satisfies never;
    }
  }
}

function toApiMessage(message: ChatMessage) {
  switch (message.role) {
    case MessageRole.System: {
      return { role: "system", content: message.content };
    }
    case MessageRole.User: {
      if (message.contentParts && message.contentParts.length > 0) {
        return {
          role: "user",
          content: message.contentParts.map(toApiContentItem),
        };
      }
      return { role: "user", content: message.content };
    }
    case MessageRole.Assistant: {
      const base: {
        role: "assistant";
        content: string;
        tool_calls?: unknown[];
        reasoning_details?: ReasoningDetails;
        model?: string;
      } = {
        role: "assistant",
        content: message.content,
      };
      if (message.model !== undefined) {
        base.model = message.model;
      }
      if (hasReasoningDetails(message.reasoningDetails)) {
        base.reasoning_details = message.reasoningDetails;
      }
      if (message.toolCalls && message.toolCalls.length > 0) {
        return { ...base, tool_calls: [...message.toolCalls] };
      }
      return base;
    }
    case MessageRole.Tool: {
      return {
        role: "tool",
        content: message.content,
        tool_call_id: message.toolCallId ?? "",
      };
    }
    default: {
      return message.role satisfies never;
    }
  }
}

function decodeResult(
  raw: unknown,
  startedAt: number,
  identifiers: ResponseIdentifiers
): Effect<ModelOutput, ModelError> {
  const parseResult = parseSchema(
    ChatResult$inboundSchema,
    normalizeResultForSchema(raw)
  );
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
  const result = parseResult.right;
  const responseIdentifiers: ModelErrorIdentifiers = {
    ...identifiers,
    ...(result.id !== undefined && { generationId: result.id }),
  };
  const choice = result.choices[0];
  if (!choice) {
    return fail(
      new ModelError({
        message: appendModelErrorIdentifiers(
          "OpenRouter response had no choices",
          responseIdentifiers
        ),
        ...responseIdentifiers,
      })
    );
  }
  const rawContent = choice.message.content;
  const completion = typeof rawContent === "string" ? rawContent : "";
  const reasoning = choice.message.reasoning ?? undefined;
  const reasoningDetails = extractReasoningDetails(raw);
  const usage = toModelUsage(result.usage);
  const toolCalls = choice.message.toolCalls ?? [];
  return recordGenerationId(result.id).pipe(
    flatMap(() =>
      succeed({
        completion,
        message: {
          role: MessageRole.Assistant,
          content: completion,
          ...(toolCalls.length > 0 && { toolCalls }),
          ...(reasoning !== undefined && { reasoning }),
          ...(reasoningDetails !== undefined && { reasoningDetails }),
          ...(result.model !== undefined && { model: result.model }),
        },
        generationTimeMs: Math.round(performance.now() - startedAt),
        ...(usage && { usage }),
      })
    )
  );
}

function normalizeResultForSchema(raw: unknown): unknown {
  if (!isRecord(raw)) {
    return raw;
  }
  return {
    ...raw,
    ...(!("system_fingerprint" in raw) && { system_fingerprint: null }),
    ...(!("created" in raw) && { created: 0 }),
    ...(!("object" in raw) && { object: "chat.completion" }),
  };
}

function extractReasoningDetails(raw: unknown): ReasoningDetails | undefined {
  if (!isRecord(raw) || !Array.isArray(raw["choices"])) {
    return undefined;
  }
  const choice = raw["choices"][0];
  if (!isRecord(choice) || !isRecord(choice["message"])) {
    return undefined;
  }
  const details = choice["message"]["reasoning_details"];
  return hasReasoningDetails(details) ? details : undefined;
}

function toModelUsage(usage: ChatUsage | undefined): ModelUsage | undefined {
  if (!usage) {
    return undefined;
  }
  const reasoningTokens = usage.completionTokensDetails?.reasoningTokens;
  return {
    inputTokens: usage.promptTokens,
    outputTokens: usage.completionTokens,
    totalTokens: usage.totalTokens,
    ...(isDefinedAndNotNull(reasoningTokens) && { reasoningTokens }),
    ...(isDefinedAndNotNull(usage.cost) && { totalCost: usage.cost }),
  };
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
