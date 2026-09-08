import { HTTPClient } from "@openrouter/sdk/lib/http";
import type {
  OpenResponsesResult,
  ResponsesRequest,
  StreamEvents,
} from "@openrouter/sdk/models";
import {
  ConnectionError,
  InvalidRequestError,
  RequestAbortedError,
  RequestTimeoutError,
  UnexpectedClientError,
} from "@openrouter/sdk/models/errors/httpclienterrors";
import { OpenRouterError } from "@openrouter/sdk/models/errors/openroutererror";
import { SDKValidationError } from "@openrouter/sdk/models/errors/sdkvalidationerror";
import { streamEventsFromJSON } from "@openrouter/sdk/models/streamevents";
import { Responses as ResponsesClient } from "@openrouter/sdk/sdk/responses";
import { Tag } from "effect/Context";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { all, fail, flatMap, map, sync, tap, tryPromise } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";

import type { Citation, ModelUsage } from "../harness/core";
import { ModelError } from "../harness/core";
import type { ReasoningDetails } from "../harness/reasoning-details";
import { Either } from "../internal/either";
import { definedValues, isRecord } from "../internal/guards";
import { parseSchema, z } from "../internal/zod";
import { filterTraceHeaders } from "../runner/trace-headers";
import { recordGenerationId } from "../runtime/generation-ids";
import type { ResponseCacheAttemptState } from "../runtime/response-cache";
import {
  buildResponseCacheSalt,
  getCurrentCallSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  getCurrentRunAttempt,
  logUnexpectedResponseCacheMiss,
  RESPONSE_CACHE_HEADER,
  RESPONSE_CACHE_SALT_HEADER,
  RESPONSE_CACHE_SOURCE_ID_HEADER,
  RESPONSE_CACHE_STATUS_HEADER,
  RESPONSE_CACHE_STATUS_HIT,
  RESPONSE_CACHE_TTL_HEADER,
  RESPONSE_CACHE_TTL_SECONDS,
} from "../runtime/response-cache";
import {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
} from "./app-identity";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import {
  appendModelErrorIdentifiers,
  modelErrorIdentifiersFromFetchHeaders,
  pickModelErrorIdentifiers,
} from "./request-identifiers";

export const ResponsesResultSchema = z.object({
  id: z.string().nullable(),
  model: z.string().nullable(),
  status: z.string().nullable(),
  output: z.array(z.record(z.string(), z.unknown())).default([]),
  usage: z.record(z.string(), z.unknown()).nullable(),
  text: z.string().default(""),
  generationId: z.string().nullable(),
  provider: z.string().nullable(),
  generationTimeMs: z.number().default(0),
});

export type ResponsesResult = z.infer<typeof ResponsesResultSchema>;

export interface ResponsesReasoning {
  readonly reasoning?: string;
  readonly reasoningDetails?: ReasoningDetails;
}

const RawResponsesTerminalEventSchema = z.object({
  type: z.union([
    z.literal("response.completed"),
    z.literal("response.incomplete"),
  ]),
  response: z
    .object({
      id: z.string(),
      model: z.string(),
      output: z.array(z.record(z.string(), z.unknown())),
      status: z.string(),
      usage: z.record(z.string(), z.unknown()).nullable().optional(),
    })
    .passthrough(),
  sequence_number: z.number().int().optional(),
});

type RawResponsesTerminalEvent = z.infer<
  typeof RawResponsesTerminalEventSchema
>;

export interface ResponsesSendOptions {
  readonly timeoutMs?: number;
  readonly versionOverride?: string;
  readonly extraHeaders?: Readonly<Record<string, string>>;
  readonly extraBody?: Readonly<Record<string, unknown>>;
  readonly onResponseIdentifiers?: (identifiers: ModelErrorIdentifiers) => void;
  readonly onStreamEvent?: (event: StreamEvents) => void;
}

export interface ResponsesConfig {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly traceHeaders?: Readonly<Record<string, string>>;
}

export const VERSION_OVERRIDE_HEADER =
  "Cloudflare-Workers-Version-Overrides" as const;

export class ResponsesError extends TaggedError("ResponsesError")<
  {
    readonly message: string;
    readonly status?: number;
    readonly retryAfterMs?: number;
    readonly retryable: boolean;
  } & ModelErrorIdentifiers
> {}

export function toModelError(error: ResponsesError): ModelError {
  const status = error.status ?? (error.retryable ? 500 : undefined);
  return new ModelError(
    definedValues({
      message: error.message,
      status,
      retryAfterMs: error.retryAfterMs,
      ...pickModelErrorIdentifiers(error),
    })
  );
}

export class Responses extends Tag(
  "@openrouter/bench-harness/responses-client/Responses"
)<
  Responses,
  {
    readonly send: (
      body: ResponsesRequest,
      options: ResponsesSendOptions
    ) => Effect<ResponsesResult, ResponsesError>;
  }
>() {}

export type ResponsesService = {
  readonly send: (
    body: ResponsesRequest,
    options: ResponsesSendOptions
  ) => Effect<ResponsesResult, ResponsesError>;
};

function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.replace(/\/+$/, "");
  return trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
}

export function makeResponsesLayer(config: ResponsesConfig): Layer<Responses> {
  const traceHeaders = filterTraceHeaders(config.traceHeaders);
  const send = (
    body: ResponsesRequest,
    options: ResponsesSendOptions,
    attemptState: ResponseCacheAttemptState
  ): Effect<ResponsesResult, ResponsesError> => {
    let identifiers: ModelErrorIdentifiers = {};
    let isCacheHit = false;
    let cacheStatus: string | undefined;
    let cacheSourceId: string | undefined;
    const httpClient = new HTTPClient({
      fetcher: async (input, init) => {
        const request = await mergeExtraBody(input, init, options.extraBody);
        const response = await fetch(request);
        identifiers = modelErrorIdentifiersFromFetchHeaders(response.headers);
        cacheStatus =
          response.headers.get(RESPONSE_CACHE_STATUS_HEADER) ?? undefined;
        isCacheHit = cacheStatus === RESPONSE_CACHE_STATUS_HIT;
        cacheSourceId =
          response.headers.get(RESPONSE_CACHE_SOURCE_ID_HEADER) ?? undefined;
        options.onResponseIdentifiers?.(identifiers);
        return response;
      },
    });
    const client = new ResponsesClient({
      apiKey: config.apiKey,
      httpClient,
      retryConfig: { strategy: "none" },
      ...definedValues({
        serverURL:
          config.baseUrl !== undefined
            ? normalizeBaseUrl(config.baseUrl)
            : undefined,
      }),
    });
    const headers: Record<string, string> = {
      "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
      "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
      ...traceHeaders,
      ...options.extraHeaders,
      ...definedValues({
        [VERSION_OVERRIDE_HEADER]: options.versionOverride
          ? `api="${options.versionOverride}"`
          : undefined,
      }),
      ...definedValues({
        "x-session-id": config.sessionId,
        [RESPONSE_CACHE_SALT_HEADER]: attemptState.cacheSalt,
      }),
      [RESPONSE_CACHE_HEADER]: "true",
      [RESPONSE_CACHE_TTL_HEADER]: `${RESPONSE_CACHE_TTL_SECONDS}`,
    };
    return tryPromise({
      try: async (signal) => {
        identifiers = {};
        const requestBody = {
          ...body,
          ...definedValues({
            cacheControl:
              body.cacheControl === undefined &&
              options.extraBody?.["cache_control"] === undefined
                ? { type: "ephemeral" as const }
                : undefined,
          }),
          stream: true,
        } satisfies ResponsesRequest;
        const stream = await client.send(
          { responsesRequest: requestBody },
          definedValues({
            fetchOptions: { signal },
            timeoutMs: options.timeoutMs,
            headers,
          })
        );
        if (!isAsyncIterable(stream)) {
          throw new ResponsesError({
            message: "Expected streaming responses result from SDK",
            retryable: false,
          });
        }
        return consumeStream(stream, options.onStreamEvent, identifiers);
      },
      catch: (cause) => toResponsesError(cause, identifiers),
    }).pipe(
      tap(() =>
        sync(() => {
          logUnexpectedResponseCacheMiss({
            ...attemptState,
            isCacheHit,
            ...definedValues({
              model: typeof body.model === "string" ? body.model : undefined,
            }),
            ...definedValues({
              cacheStatus,
            }),
            ...identifiers,
          });
        })
      ),
      flatMap((result) =>
        result
          ? recordGenerationId(
              isCacheHit && cacheSourceId !== undefined
                ? cacheSourceId
                : result.generationId,
              isCacheHit,
              isCacheHit && cacheSourceId !== undefined
            ).pipe(map(() => result))
          : fail(
              new ResponsesError({
                message: appendModelErrorIdentifiers(
                  "Stream ended without a response.completed event",
                  identifiers
                ),
                retryable: true,
                ...identifiers,
              })
            )
      )
    );
  };
  const sendWithCacheSalt = (
    body: ResponsesRequest,
    options: ResponsesSendOptions
  ): Effect<ResponsesResult, ResponsesError> => {
    return all({
      epoch: getCurrentEpoch,
      retryAttempt: getCurrentRetryAttempt,
      runAttempt: getCurrentRunAttempt,
      callSalt: getCurrentCallSalt,
    }).pipe(
      flatMap(({ epoch, retryAttempt, runAttempt, callSalt }) => {
        const cacheSalt = buildResponseCacheSalt(
          config.sessionId,
          epoch,
          retryAttempt,
          callSalt
        );
        return send(body, options, { runAttempt, retryAttempt, cacheSalt });
      })
    );
  };
  return layerSucceed(Responses, Responses.of({ send: sendWithCacheSalt }));
}

async function mergeExtraBody(
  input: string | URL | Request,
  init: RequestInit | undefined,
  extraBody: Readonly<Record<string, unknown>> | undefined
): Promise<Request> {
  const request = input instanceof Request ? input : new Request(input, init);
  if (extraBody === undefined) {
    return request;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(await request.clone().text());
  } catch {
    return request;
  }
  if (!isRecord(parsed)) {
    return request;
  }
  return new Request(request, {
    method: "POST",
    body: JSON.stringify({
      ...parsed,
      ...extraBody,
    }),
  });
}

export function unwrapStreamEvent(event: unknown): unknown {
  return isRecord(event) && isRecord(event["raw"]) ? event["raw"] : event;
}

function isAsyncIterable(value: unknown): value is AsyncIterable<StreamEvents> {
  return (
    typeof value === "object" &&
    value !== null &&
    Symbol.asyncIterator in value &&
    typeof value[Symbol.asyncIterator] === "function"
  );
}

function normalizeRawTerminalEvent(
  event: RawResponsesTerminalEvent
): RawResponsesTerminalEvent {
  const usage = event.response.usage;
  return {
    ...event,
    sequence_number: event.sequence_number ?? 0,
    response: {
      ...event.response,
      completed_at: event.response["completed_at"] ?? null,
      created_at: event.response["created_at"] ?? 0,
      error: event.response["error"] ?? null,
      frequency_penalty: event.response["frequency_penalty"] ?? null,
      incomplete_details: event.response["incomplete_details"] ?? null,
      instructions: event.response["instructions"] ?? null,
      metadata: event.response["metadata"] ?? null,
      parallel_tool_calls: event.response["parallel_tool_calls"] ?? false,
      presence_penalty: event.response["presence_penalty"] ?? null,
      temperature: event.response["temperature"] ?? null,
      tool_choice: event.response["tool_choice"] ?? "auto",
      tools: event.response["tools"] ?? [],
      top_p: event.response["top_p"] ?? null,
      ...definedValues({
        usage:
          usage !== undefined && usage !== null
            ? {
                ...usage,
                input_tokens_details: usage["input_tokens_details"] ?? {
                  cached_tokens: 0,
                },
                output_tokens_details: usage["output_tokens_details"] ?? {
                  reasoning_tokens: 0,
                },
              }
            : undefined,
      }),
    },
  };
}

export async function consumeStream(
  stream: AsyncIterable<StreamEvents>,
  onEvent?: (event: StreamEvents) => void,
  initialIdentifiers: ModelErrorIdentifiers = {}
): Promise<ResponsesResult | null> {
  let finalResponse: OpenResponsesResult | null = null;
  const startedAt = performance.now();
  const identifiers = initialIdentifiers;
  try {
    for await (const event of stream) {
      onEvent?.(event);
      const rawEvent = unwrapStreamEvent(event);
      const eventResponse = isRecord(rawEvent)
        ? rawEvent["response"]
        : undefined;
      if (isRecord(eventResponse) && typeof eventResponse["id"] === "string") {
        Object.assign(identifiers, { generationId: eventResponse["id"] });
      }
      const rawType = isRecord(rawEvent) ? rawEvent["type"] : undefined;
      if (rawType === "response.failed") {
        throw new ResponsesError({
          message: appendModelErrorIdentifiers(
            `OpenRouter stream error: ${extractResponseError(eventResponse)}`,
            identifiers
          ),
          retryable: true,
          ...identifiers,
        });
      }
      if (rawType === "error") {
        throw new ResponsesError({
          message: appendModelErrorIdentifiers(
            `OpenRouter stream error: ${isRecord(rawEvent) ? String(rawEvent["message"]) : String(rawEvent)}`,
            identifiers
          ),
          retryable: true,
          ...identifiers,
        });
      }
      switch (event.type) {
        case "response.completed":
        case "response.incomplete": {
          finalResponse = event.response;
          break;
        }
        default: {
          const parsedRawEvent = parseSchema(
            RawResponsesTerminalEventSchema,
            rawEvent
          );
          if (Either.isLeft(parsedRawEvent)) {
            break;
          }
          const parsedTerminalEvent = streamEventsFromJSON(
            JSON.stringify(normalizeRawTerminalEvent(parsedRawEvent.right))
          );
          if (!parsedTerminalEvent.ok) {
            break;
          }
          switch (parsedTerminalEvent.value.type) {
            case "response.completed": {
              finalResponse = parsedTerminalEvent.value.response;
              break;
            }
            case "response.incomplete": {
              finalResponse = parsedTerminalEvent.value.response;
              break;
            }
          }
        }
      }
    }
  } catch (cause) {
    if (cause instanceof ResponsesError) {
      throw cause;
    }
    throw toResponsesError(cause, identifiers);
  }
  if (finalResponse === null) {
    return null;
  }
  const { output } = finalResponse;
  const usage = finalResponse.usage ?? null;
  const { id } = finalResponse;
  const providerRaw: unknown = finalResponse;
  const provider =
    isRecord(providerRaw) && typeof providerRaw["provider"] === "string"
      ? providerRaw["provider"]
      : null;
  return {
    id,
    model: finalResponse.model,
    status: finalResponse.status,
    output,
    usage,
    text: extractMessageText(output),
    generationId: id,
    provider,
    generationTimeMs: Math.round(performance.now() - startedAt),
  };
}

export function usageFromResponses(
  usage: Readonly<Record<string, unknown>> | null
): ModelUsage | undefined {
  if (usage === null) {
    return undefined;
  }
  const detailsRaw =
    usage["outputTokensDetails"] ?? usage["output_tokens_details"];
  const details = isRecord(detailsRaw) ? detailsRaw : undefined;
  const inputTokens = numField(usage, "inputTokens", "input_tokens");
  const outputTokens = numField(usage, "outputTokens", "output_tokens");
  const totalTokens = numField(usage, "totalTokens", "total_tokens");
  const reasoningTokens =
    details !== undefined
      ? numField(details, "reasoningTokens", "reasoning_tokens")
      : undefined;
  const totalCost = numField(usage, "cost");
  const serverToolUseRaw =
    usage["serverToolUseDetails"] ?? usage["server_tool_use_details"];
  const serverToolUse = isRecord(serverToolUseRaw)
    ? serverToolUseRaw
    : undefined;
  const webSearchRequests =
    serverToolUse !== undefined
      ? numField(serverToolUse, "webSearchRequests", "web_search_requests")
      : undefined;
  const toolCallsRequested =
    serverToolUse !== undefined
      ? numField(serverToolUse, "toolCallsRequested", "tool_calls_requested")
      : undefined;
  const toolCallsExecuted =
    serverToolUse !== undefined
      ? numField(serverToolUse, "toolCallsExecuted", "tool_calls_executed")
      : undefined;
  const hasServerToolUse =
    webSearchRequests !== undefined ||
    toolCallsRequested !== undefined ||
    toolCallsExecuted !== undefined;
  return definedValues({
    inputTokens,
    outputTokens,
    totalTokens,
    reasoningTokens,
    totalCost,
    serverToolUse: hasServerToolUse
      ? definedValues({
          webSearchRequests,
          toolCallsRequested,
          toolCallsExecuted,
        })
      : undefined,
  });
}

function numField(
  record: Readonly<Record<string, unknown>>,
  ...keys: readonly string[]
): number | undefined {
  for (const key of keys) {
    const v = record[key];
    if (typeof v === "number") {
      return v;
    }
  }
  return undefined;
}

export function extractMessageText(
  output: readonly Record<string, unknown>[]
): string {
  let text = "";
  for (const item of output) {
    if (item["type"] !== "message") {
      continue;
    }
    const { content } = item;
    if (!Array.isArray(content)) {
      continue;
    }
    for (const part of content) {
      if (
        part !== null &&
        typeof part === "object" &&
        part["type"] === "output_text" &&
        typeof part["text"] === "string"
      ) {
        text += part["text"];
      }
    }
  }
  return text;
}

export function extractReasoning(
  output: readonly Record<string, unknown>[]
): ResponsesReasoning {
  const details: unknown[] = [];
  const texts: string[] = [];
  const summaries: string[] = [];
  for (const item of output) {
    if (item["type"] !== "reasoning") {
      continue;
    }
    const id = stringField(item, "id");
    const format = stringField(item, "format");
    const signature = stringField(item, "signature");
    for (const text of partTexts(item["content"], "reasoning_text")) {
      texts.push(text);
      details.push(
        definedValues({
          type: "reasoning.text",
          text,
          id,
          format,
          signature,
        })
      );
    }
    for (const summary of partTexts(item["summary"], "summary_text")) {
      summaries.push(summary);
      details.push(
        definedValues({
          type: "reasoning.summary",
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
          type: "reasoning.encrypted",
          data: encrypted,
          id,
          format,
        })
      );
    }
  }
  const readable = texts.length > 0 ? texts : summaries;
  return definedValues({
    reasoning: readable.length > 0 ? readable.join("\n\n") : undefined,
    reasoningDetails: details.length > 0 ? details : undefined,
  });
}

function partTexts(parts: unknown, partType: string): string[] {
  if (!Array.isArray(parts)) {
    return [];
  }
  return parts.flatMap((part) => {
    if (!isRecord(part) || part["type"] !== partType) {
      return [];
    }
    const text = stringField(part, "text");
    return text !== undefined ? [text] : [];
  });
}

function stringField(
  record: Readonly<Record<string, unknown>>,
  key: string
): string | undefined {
  const value = record[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

export function findOutputItems(
  output: readonly Record<string, unknown>[],
  itemType: string
): Record<string, unknown>[] {
  return output.filter((item) => item["type"] === itemType);
}

export function extractCitations(
  output: readonly Record<string, unknown>[]
): Citation[] {
  const citations: Citation[] = [];
  for (const item of output) {
    if (item["type"] !== "message") {
      continue;
    }
    const { content } = item;
    if (!Array.isArray(content)) {
      continue;
    }
    for (const part of content) {
      if (
        part === null ||
        typeof part !== "object" ||
        part["type"] !== "output_text"
      ) {
        continue;
      }
      const { annotations } = part;
      if (!Array.isArray(annotations)) {
        continue;
      }
      for (const ann of annotations) {
        if (
          isRecord(ann) &&
          ann["type"] === "url_citation" &&
          typeof ann["url"] === "string" &&
          typeof ann["title"] === "string"
        ) {
          citations.push({
            url: ann["url"],
            title: ann["title"],
            startIndex:
              typeof ann["start_index"] === "number" ? ann["start_index"] : 0,
            endIndex:
              typeof ann["end_index"] === "number" ? ann["end_index"] : 0,
          });
        }
      }
    }
  }
  return citations;
}

function extractResponseError(response: unknown): string {
  if (isRecord(response)) {
    const err = response["error"];
    if (isRecord(err) && typeof err["message"] === "string") {
      return err["message"];
    }
    if (typeof response["message"] === "string") {
      return response["message"];
    }
  }
  return String(response);
}

function toResponsesError(
  cause: unknown,
  identifiers: ModelErrorIdentifiers = {}
): ResponsesError {
  if (cause instanceof ResponsesError) {
    return cause;
  }
  if (cause instanceof OpenRouterError) {
    const errorIdentifiers = {
      ...identifiers,
      ...modelErrorIdentifiersFromFetchHeaders(cause.headers),
    };
    const retryAfterMs = parseRetryAfter(cause.headers.get("retry-after"));
    return new ResponsesError(
      definedValues({
        message: appendModelErrorIdentifiers(
          `OpenRouter HTTP ${cause.statusCode}: ${cause.body}`,
          errorIdentifiers
        ),
        status: cause.statusCode,
        retryAfterMs,
        retryable: cause.statusCode === 429 || cause.statusCode >= 500,
        ...errorIdentifiers,
      })
    );
  }
  if (
    cause instanceof RequestAbortedError ||
    cause instanceof RequestTimeoutError
  ) {
    return new ResponsesError({
      message: appendModelErrorIdentifiers(cause.message, identifiers),
      status: 408,
      retryable: true,
      ...identifiers,
    });
  }
  if (
    cause instanceof ConnectionError ||
    cause instanceof UnexpectedClientError ||
    cause instanceof SDKValidationError
  ) {
    return new ResponsesError({
      message: appendModelErrorIdentifiers(cause.message, identifiers),
      status: 500,
      retryable: true,
      ...identifiers,
    });
  }
  if (cause instanceof InvalidRequestError) {
    return new ResponsesError({
      message: appendModelErrorIdentifiers(cause.message, identifiers),
      status: 400,
      retryable: false,
      ...identifiers,
    });
  }
  if (cause instanceof SyntaxError || cause instanceof z.ZodError) {
    return new ResponsesError({
      message: appendModelErrorIdentifiers(cause.message, identifiers),
      status: 500,
      retryable: true,
      ...identifiers,
    });
  }
  if (cause instanceof TypeError) {
    return new ResponsesError({
      message: appendModelErrorIdentifiers(
        `Network error: ${cause.message}`,
        identifiers
      ),
      retryable: true,
      ...identifiers,
    });
  }
  if (
    cause instanceof DOMException &&
    (cause.name === "AbortError" || cause.name === "TimeoutError")
  ) {
    return new ResponsesError({
      message: appendModelErrorIdentifiers(
        "Wall-clock timeout (request aborted)",
        identifiers
      ),
      status: 408,
      retryable: true,
      ...identifiers,
    });
  }
  return new ResponsesError({
    message: appendModelErrorIdentifiers(
      `OpenRouter request failed: ${String(cause)}`,
      identifiers
    ),
    retryable: false,
    ...identifiers,
  });
}

function parseRetryAfter(value: string | null): number | undefined {
  if (value === null) {
    return undefined;
  }
  const seconds = Number(value);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds * 1e3 : undefined;
}
