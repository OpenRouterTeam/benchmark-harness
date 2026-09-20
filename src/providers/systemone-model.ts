import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  all,
  catchTag,
  fail,
  flatMap,
  gen,
  map,
  suspend,
  timeout,
  tryPromise,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";

import type { ModelMessage, ModelOutput } from "../harness/core";
import { MessageRole, ModelError } from "../harness/core";
import type { GenerateConfig } from "../harness/model";
import { Model } from "../harness/model";
import { Either } from "../internal/either";
import { definedValues } from "../internal/guards";
import { parseSchema, z } from "../internal/zod";
import { filterTraceHeaders } from "../runner/trace-headers";
import { recordGenerationId } from "../runtime/generation-ids";
import {
  buildRequestSessionId,
  getCurrentSampleId,
} from "../runtime/request-session-id";
import { getCurrentEpoch } from "../runtime/response-cache";
import type { RetryConfig } from "../runtime/retry";
import { rateLimitRetrySchedule, retrySalted } from "../runtime/retry";
import {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
} from "./app-identity";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import {
  appendModelErrorIdentifiers,
  modelErrorIdentifiersFromFetchHeaders,
} from "./request-identifiers";

export const SYSTEMONE_PATH = "/systemone";

export const SystemOneQuestionSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("noul"),
    instructions: z.string(),
    criteria: z.object({ true: z.string(), false: z.string() }).optional(),
  }),
  z.object({
    type: z.literal("choice"),
    instructions: z.string(),
    criteria: z.record(z.string(), z.string().nullable()),
  }),
  z.object({
    type: z.literal("score"),
    instructions: z.string(),
    criteria: z.array(z.string()),
  }),
]);

export type SystemOneQuestion = z.infer<typeof SystemOneQuestionSchema>;

export const SystemOneRequestSchema = z.object({
  state: z.union([
    z.string(),
    z.record(z.string(), z.unknown()),
    z.array(z.unknown()),
  ]),
  questions: z.record(z.string(), SystemOneQuestionSchema),
});

export type SystemOneRequest = z.infer<typeof SystemOneRequestSchema>;

const UnitIntervalSchema = z.number().min(0).max(1);

export const SystemOneAnswerSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("noul"), noul: UnitIntervalSchema }),
  z.object({
    type: z.literal("choice"),
    choice: z.string(),
    probabilities: z.record(z.string(), UnitIntervalSchema).optional(),
    confidence: UnitIntervalSchema.optional(),
  }),
  z.object({
    type: z.literal("score"),
    score: z.number(),
    legend: z.record(z.string(), z.string()).optional(),
    probabilities: z.record(z.string(), UnitIntervalSchema).optional(),
    confidence: UnitIntervalSchema.optional(),
  }),
]);

export type SystemOneAnswer = z.infer<typeof SystemOneAnswerSchema>;

export const SystemOneAnswersSchema = z.record(
  z.string(),
  SystemOneAnswerSchema
);

export type SystemOneAnswers = z.infer<typeof SystemOneAnswersSchema>;

export const SystemOneResponseSchema = z.object({
  id: z.string().optional(),
  model: z.string().optional(),
  provider: z.string().optional(),
  answers: SystemOneAnswersSchema,
  usage: z
    .object({
      input_tokens: z.number().optional(),
      output_tokens: z.number().optional(),
      cost: z.number().optional(),
    })
    .optional(),
});

export type SystemOneResponse = z.infer<typeof SystemOneResponseSchema>;

export function systemOneRequestMessage(
  request: SystemOneRequest
): ModelMessage {
  return { role: MessageRole.User, content: JSON.stringify(request) };
}

export function parseSystemOneRequest(
  messages: readonly ModelMessage[]
): Either.Either<SystemOneRequest, ModelError> {
  const last = messages.findLast(
    (message) => message.role === MessageRole.User
  );
  if (last === undefined) {
    return Either.left(
      new ModelError({ message: "SystemOne request has no user message" })
    );
  }
  const json = Either.try((): unknown => JSON.parse(last.content));
  if (Either.isLeft(json)) {
    return Either.left(
      new ModelError({
        message: "SystemOne request message is not JSON",
      })
    );
  }
  const parsed = parseSchema(SystemOneRequestSchema, json.right);
  if (Either.isLeft(parsed)) {
    return Either.left(
      new ModelError({
        message: `SystemOne request failed validation: ${parsed.left.message}`,
      })
    );
  }
  return Either.right(parsed.right);
}

export function parseSystemOneAnswers(
  completion: string
): Either.Either<SystemOneAnswers, string> {
  const json = Either.try((): unknown => JSON.parse(completion));
  if (Either.isLeft(json)) {
    return Either.left("completion is not JSON");
  }
  const parsed = parseSchema(SystemOneAnswersSchema, json.right);
  return Either.isLeft(parsed)
    ? Either.left(parsed.left.message)
    : Either.right(parsed.right);
}

export interface SystemOneModelConfig {
  readonly model: string;
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly retry?: RetryConfig;
  readonly traceHeaders?: Readonly<Record<string, string>>;
}

function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.replace(/\/+$/u, "");
  return trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
}

const RETRYABLE_STATUS_FALLBACK = 500;

function toModelOutput(
  response: SystemOneResponse,
  raw: Record<string, unknown>,
  generationTimeMs: number
): ModelOutput {
  const completion = JSON.stringify(response.answers);
  const inputTokens = response.usage?.input_tokens;
  const outputTokens = response.usage?.output_tokens;
  return definedValues({
    completion,
    message: definedValues({
      role: MessageRole.Assistant,
      content: completion,
      model: response.model,
    }),
    usage: definedValues({
      inputTokens,
      outputTokens,
      totalTokens:
        inputTokens !== undefined && outputTokens !== undefined
          ? inputTokens + outputTokens
          : undefined,
      totalCost: response.usage?.cost,
    }),
    generationTimeMs,
    rawResponse: raw,
  });
}

function sendSystemOne(opts: {
  readonly url: string;
  readonly headers: Readonly<Record<string, string>>;
  readonly body: Readonly<Record<string, unknown>>;
  readonly timeoutMs: number | undefined;
  readonly retry: RetryConfig | undefined;
}): Effect<ModelOutput, ModelError> {
  let identifiers: ModelErrorIdentifiers = {};
  const attempt = suspend(() => {
    identifiers = {};
    const startedAt = performance.now();
    return tryPromise({
      try: async (signal) => {
        const response = await fetch(opts.url, {
          method: "POST",
          headers: { ...opts.headers, "Content-Type": "application/json" },
          body: JSON.stringify(opts.body),
          signal,
        });
        identifiers = modelErrorIdentifiersFromFetchHeaders(response.headers);
        const text = await response.text();
        return { status: response.status, text };
      },
      catch: (cause) =>
        new ModelError({
          message: appendModelErrorIdentifiers(
            `SystemOne request failed: ${String(cause)}`,
            identifiers
          ),
          status: RETRYABLE_STATUS_FALLBACK,
          ...identifiers,
        }),
    }).pipe(
      flatMap(({ status, text }) => {
        if (status < 200 || status >= 300) {
          return fail(
            new ModelError({
              status,
              message: appendModelErrorIdentifiers(
                `SystemOne request returned HTTP ${status}: ${text.slice(0, 500)}`,
                identifiers
              ),
              ...identifiers,
            })
          );
        }
        const json = Either.try((): unknown => JSON.parse(text));
        if (Either.isLeft(json)) {
          return fail(
            new ModelError({
              status: RETRYABLE_STATUS_FALLBACK,
              message: appendModelErrorIdentifiers(
                "SystemOne response was not JSON",
                identifiers
              ),
              ...identifiers,
            })
          );
        }
        const parsed = parseSchema(SystemOneResponseSchema, json.right);
        if (Either.isLeft(parsed)) {
          return fail(
            new ModelError({
              status: RETRYABLE_STATUS_FALLBACK,
              message: appendModelErrorIdentifiers(
                `SystemOne response failed validation: ${parsed.left.message}`,
                identifiers
              ),
              ...identifiers,
            })
          );
        }
        const raw = parseSchema(z.record(z.string(), z.unknown()), json.right);
        return recordGenerationId(parsed.right.id).pipe(
          map(() =>
            toModelOutput(
              parsed.right,
              Either.isRight(raw) ? raw.right : {},
              Math.round(performance.now() - startedAt)
            )
          )
        );
      })
    );
  });
  const timeoutMs = opts.timeoutMs;
  const timed =
    timeoutMs !== undefined && timeoutMs > 0
      ? attempt.pipe(
          timeout(millis(timeoutMs)),
          catchTag("TimeoutException", () =>
            fail(
              new ModelError({
                status: 408,
                message: appendModelErrorIdentifiers(
                  `SystemOne request timed out after ${timeoutMs}ms`,
                  identifiers
                ),
                ...identifiers,
              })
            )
          )
        )
      : attempt;
  return retrySalted(timed, rateLimitRetrySchedule(opts.retry ?? {}));
}

export function makeSystemOneModelLayer(
  config: SystemOneModelConfig
): Layer<Model> {
  const url = `${normalizeBaseUrl(config.baseUrl ?? "https://openrouter.ai/api/v1")}${SYSTEMONE_PATH}`;
  const traceHeaders = filterTraceHeaders(config.traceHeaders);
  const generate = (
    messages: readonly ModelMessage[],
    generateConfig: GenerateConfig
  ): Effect<ModelOutput, ModelError> =>
    gen(function* () {
      const request = yield* parseSystemOneRequest(messages);
      const { epoch, sampleId } = yield* all({
        epoch: getCurrentEpoch,
        sampleId: getCurrentSampleId,
      });
      const headers: Record<string, string> = {
        Authorization: `Bearer ${config.apiKey}`,
        "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
        "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
        ...traceHeaders,
        ...definedValues({
          "x-session-id": buildRequestSessionId(
            config.sessionId,
            epoch,
            sampleId
          ),
          "X-OR-Endpoint-Id": generateConfig.endpointId,
        }),
      };
      return yield* sendSystemOne({
        url,
        headers,
        body: {
          model: config.model,
          state: request.state,
          questions: request.questions,
          ...generateConfig.extraBody,
        },
        timeoutMs: generateConfig.timeoutMs,
        retry: config.retry,
      });
    });
  return layerSucceed(Model, Model.of({ generate }));
}
