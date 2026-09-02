import type {
  EasyInputMessage,
  InputsUnion,
  ResponsesRequest,
  StreamEvents,
} from "@openrouter/sdk/models";
import { camelCase, snakeCase } from "change-case";
import { Tag } from "effect/Context";
import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  catchTag,
  fail,
  gen,
  map,
  mapError,
  suspend,
  timeout,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";

import type { ModelUsage } from "../harness/core";
import { ModelError } from "../harness/core";
import type { GenerateConfig } from "../harness/model";
import { stripVariantSuffix } from "../harness/model";
import { definedValues, isRecord } from "../internal/guards";
import type { RetryConfig } from "../runtime/retry";
import { rateLimitRetrySchedule, retrySalted } from "../runtime/retry";
import { buildAutoRouterPlugin } from "./auto-router-plugin";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import { appendModelErrorIdentifiers } from "./request-identifiers";
import type { ResponsesResult, ResponsesService } from "./responses-client";
import {
  extractMessageText,
  makeResponsesLayer,
  Responses,
  toModelError,
  unwrapStreamEvent,
  usageFromResponses,
} from "./responses-client";

export type ResponsesInputItem = Record<string, unknown>;

export type ResponsesMessageRole =
  | "user"
  | "assistant"
  | "system"
  | "developer";

export function responsesMessage(
  role: ResponsesMessageRole,
  content: string
): EasyInputMessage {
  return { type: "message", role, content };
}

export interface ResponsesFunctionTool {
  readonly type: "function";
  readonly name: string;
  readonly description?: string;
  readonly parameters: Record<string, unknown>;
  readonly strict?: boolean | null;
}

export interface ResponsesGenerateConfig extends Omit<GenerateConfig, "tools"> {
  readonly model?: string;
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
  readonly traceHeaders?: Readonly<Record<string, string>>;
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
  const baseUrl = config.baseUrl ?? "https://openrouter.ai/api/v1";
  const responsesLayer = makeResponsesLayer(
    definedValues({
      apiKey: config.apiKey,
      baseUrl,
      sessionId: config.sessionId,
      traceHeaders: config.traceHeaders,
    })
  );
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

export function generate(
  opts: ResponsesGenerateOpts,
  responses: ResponsesService
): Effect<ResponsesTurn, ModelError> {
  const { genConfig } = opts;
  const sendSort =
    genConfig.sort !== undefined && genConfig.endpointId === undefined;
  const providerPreferences = definedValues({
    sort: sendSort ? genConfig.sort : undefined,
    only:
      genConfig.providerOnly !== undefined
        ? [...genConfig.providerOnly]
        : undefined,
    ignore:
      genConfig.providerIgnore !== undefined
        ? [...genConfig.providerIgnore]
        : undefined,
    allowFallbacks: genConfig.allowFallbacks,
  });
  const sendProvider = Object.keys(providerPreferences).length > 0;
  const requestModel = genConfig.model ?? opts.model;
  const baseModel = stripVariantSuffix(requestModel);
  const autoRouterPlugin = buildAutoRouterPlugin(baseModel, genConfig);
  const body = {
    model: requestModel,
    input: toSdkInput(opts.input),
    store: false,
    include: ["reasoning.encrypted_content"],
    ...definedValues({
      instructions: genConfig.instructions,
      temperature: genConfig.temperature,
      maxOutputTokens: genConfig.maxTokens,
      tools:
        genConfig.tools !== undefined && genConfig.tools.length > 0
          ? [...genConfig.tools]
          : undefined,
    }),
    reasoning: { effort: genConfig.reasoningEffort },
    ...definedValues({
      provider: sendProvider ? providerPreferences : undefined,
    }),
    ...definedValues({
      plugins: autoRouterPlugin !== undefined ? [autoRouterPlugin] : undefined,
    }),
  } satisfies ResponsesRequest;
  const extraHeaders = definedValues({
    "X-OR-Endpoint-Id": genConfig.endpointId,
    "Cloudflare-Workers-Version-Overrides": genConfig.cloudflareVersion,
  });
  const extraBody = genConfig.extraBody;
  let identifiers: ModelErrorIdentifiers = {};
  const requestAttempt = suspend(() => {
    identifiers = {};
    const startedAt = performance.now();
    return responses
      .send(body, {
        ...definedValues({
          extraHeaders:
            Object.keys(extraHeaders).length > 0 ? extraHeaders : undefined,
        }),
        ...definedValues({
          extraBody,
        }),
        onResponseIdentifiers: (responseIdentifiers) => {
          identifiers = { ...identifiers, ...responseIdentifiers };
        },
        onStreamEvent: (event: StreamEvents) => {
          const rawEvent = unwrapStreamEvent(event);
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
  const timeoutMs = genConfig.timeoutMs;
  const timedAttempt =
    timeoutMs !== undefined && timeoutMs > 0
      ? requestAttempt.pipe(
          timeout(millis(timeoutMs)),
          catchTag("TimeoutException", () =>
            fail(
              new ModelError({
                status: 408,
                message: appendModelErrorIdentifiers(
                  `Request timed out after ${timeoutMs}ms`,
                  identifiers
                ),
                ...identifiers,
              })
            )
          )
        )
      : requestAttempt;
  return retrySalted(timedAttempt, rateLimitRetrySchedule(opts.retry ?? {}));
}

const RAW_PAYLOAD_KEYS = new Set(["arguments", "output"]);

export function toSdkInput(input: readonly ResponsesInputItem[]): InputsUnion {
  return input.map(toSdkValue) as InputsUnion;
}

function toSdkValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(toSdkValue);
  }
  if (!isRecord(value)) {
    return value;
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, nestedValue]) => [
      camelCase(key),
      RAW_PAYLOAD_KEYS.has(key) ? nestedValue : toSdkValue(nestedValue),
    ])
  );
}

function toWireRecord(
  record: Readonly<Record<string, unknown>>
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(record).map(([key, value]) => [
      snakeCase(key),
      RAW_PAYLOAD_KEYS.has(key) ? value : toWireValue(value),
    ])
  );
}

function toWireValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(toWireValue);
  }
  return isRecord(value) ? toWireRecord(value) : value;
}

function toResponsesTurn(
  result: ResponsesResult,
  generationTimeMs: number
): ResponsesTurn {
  const outputItems = result.output.map(toWireRecord);
  const usage = usageFromResponses(result.usage);
  const functionCalls = outputItems.flatMap((item) => {
    const callId = item["call_id"];
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
  return definedValues({
    outputItems,
    functionCalls,
    text: extractMessageText(outputItems),
    usage,
    generationTimeMs,
  });
}
