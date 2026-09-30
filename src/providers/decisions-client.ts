import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  catchTag,
  fail,
  gen,
  promise,
  timeout,
  tryPromise,
} from "effect/Effect";

import type { ModelUsage } from "../harness/core";
import { ModelError } from "../harness/core";
import type { GenerateConfig } from "../harness/model";
import { stripVariantSuffix } from "../harness/model";
import { Either } from "../internal/either";
import { definedValues } from "../internal/guards";
import { parseSchema, z } from "../internal/zod";
import { filterTraceHeaders } from "../runner/trace-headers";
import { recordGenerationId } from "../runtime/generation-ids";
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

export const DECISIONS_PATH = "/api/alpha/decisions";

const DECISIONS_MODEL_PREFIXES = ["typesafe/"] as const;

export function isDecisionsModel(model: string): boolean {
  const id = stripVariantSuffix(model).replace(/^~/u, "");
  return DECISIONS_MODEL_PREFIXES.some((prefix) => id.startsWith(prefix));
}

export function decisionsUrl(baseUrl = "https://openrouter.ai/api/v1"): string {
  return `${new URL(baseUrl).origin}${DECISIONS_PATH}`;
}

export interface DecisionsConfig {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly retry?: RetryConfig;
  readonly traceHeaders?: Readonly<Record<string, string>>;
}

export interface DecisionsChoiceRequest {
  readonly model: string;
  readonly state: unknown;
  readonly instructions: string;
  readonly criteria: Readonly<Record<string, string>>;
}

export interface DecisionsChoiceResult {
  readonly probabilities: Readonly<Record<string, number>>;
  readonly usage: ModelUsage;
  readonly generationTimeMs: number;
}

export interface DecisionsService {
  readonly choose: (
    request: DecisionsChoiceRequest,
    config: GenerateConfig
  ) => Effect<DecisionsChoiceResult, ModelError>;
}

const DecisionsChoiceResponseSchema = z.object({
  id: z.string().optional(),
  answers: z.record(
    z.string(),
    z.object({
      type: z.literal("choice"),
      choice: z.string(),
      probabilities: z.record(z.string(), z.number()).optional(),
    })
  ),
  usage: z.object({
    input_tokens: z.number().int().nonnegative(),
    output_tokens: z.number().int().nonnegative(),
    cost: z.number().optional(),
  }),
});

const QUESTION_KEY = "decision";

function providerPreferences(
  config: GenerateConfig
): Readonly<Record<string, unknown>> | undefined {
  const prefs = definedValues({
    only: config.providerOnly,
    ignore: config.providerIgnore,
    allow_fallbacks: config.allowFallbacks,
    sort: config.sort,
  });
  return Object.keys(prefs).length > 0 ? prefs : undefined;
}

function parseRetryAfterMs(value: string | null): number | undefined {
  if (value === null) {
    return undefined;
  }
  const seconds = Number(value);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds * 1e3 : undefined;
}

async function readErrorBody(response: Response): Promise<string> {
  const text = await response.text().catch(() => "");
  return text.slice(0, 2000);
}

export function makeDecisionsService(
  config: DecisionsConfig
): DecisionsService {
  const url = decisionsUrl(config.baseUrl);
  const traceHeaders = filterTraceHeaders(config.traceHeaders);

  const sendOnce = (
    request: DecisionsChoiceRequest,
    generateConfig: GenerateConfig
  ): Effect<DecisionsChoiceResult, ModelError> =>
    gen(function* () {
      const startedAt = performance.now();
      const body = {
        model: request.model,
        state: request.state,
        questions: {
          [QUESTION_KEY]: {
            type: "choice",
            instructions: request.instructions,
            criteria: request.criteria,
          },
        },
        ...definedValues({
          provider: providerPreferences(generateConfig),
          session_id: config.sessionId,
        }),
      };
      const headers: Record<string, string> = {
        authorization: `Bearer ${config.apiKey}`,
        "content-type": "application/json",
        "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
        "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
        ...traceHeaders,
        ...definedValues({ "x-session-id": config.sessionId }),
      };
      const response = yield* tryPromise({
        try: (signal) =>
          fetch(url, {
            method: "POST",
            headers,
            body: JSON.stringify(body),
            signal,
          }),
        catch: (cause) =>
          new ModelError({
            message: `Decisions request failed: ${cause instanceof Error ? cause.message : String(cause)}`,
          }),
      });
      const identifiers: ModelErrorIdentifiers =
        modelErrorIdentifiersFromFetchHeaders(response.headers);
      if (!response.ok) {
        const text = yield* promise(() => readErrorBody(response));
        return yield* fail(
          new ModelError(
            definedValues({
              message: appendModelErrorIdentifiers(
                `Decisions HTTP ${response.status}: ${text}`,
                identifiers
              ),
              status: response.status,
              retryAfterMs: parseRetryAfterMs(
                response.headers.get("retry-after")
              ),
              ...identifiers,
            })
          )
        );
      }
      const json = yield* tryPromise({
        try: (): Promise<unknown> => response.json(),
        catch: () =>
          new ModelError({
            message: appendModelErrorIdentifiers(
              "Decisions response was not JSON",
              identifiers
            ),
            status: response.status,
            ...identifiers,
          }),
      });
      const parsed = parseSchema(DecisionsChoiceResponseSchema, json);
      if (Either.isLeft(parsed)) {
        return yield* fail(
          new ModelError({
            message: appendModelErrorIdentifiers(
              `Decisions response failed validation: ${parsed.left.message}`,
              identifiers
            ),
            status: response.status,
            ...identifiers,
          })
        );
      }
      const answer = parsed.right.answers[QUESTION_KEY];
      if (answer === undefined) {
        return yield* fail(
          new ModelError({
            message: appendModelErrorIdentifiers(
              `Decisions response omitted the "${QUESTION_KEY}" answer`,
              identifiers
            ),
            status: response.status,
            ...identifiers,
          })
        );
      }
      const probabilities =
        answer.probabilities ??
        Object.fromEntries(
          Object.keys(request.criteria).map((key) => [
            key,
            key === answer.choice ? 1 : 0,
          ])
        );
      yield* recordGenerationId(parsed.right.id);
      const usage = parsed.right.usage;
      return {
        probabilities,
        usage: {
          inputTokens: usage.input_tokens,
          outputTokens: usage.output_tokens,
          totalTokens: usage.input_tokens + usage.output_tokens,
          reasoningTokens: 0,
          totalCost: usage.cost ?? 0,
        },
        generationTimeMs: Math.round(performance.now() - startedAt),
      } satisfies DecisionsChoiceResult;
    });

  const choose = (
    request: DecisionsChoiceRequest,
    generateConfig: GenerateConfig
  ): Effect<DecisionsChoiceResult, ModelError> => {
    const attempt = sendOnce(request, generateConfig);
    const timeoutMs = generateConfig.timeoutMs;
    const timed =
      timeoutMs !== undefined && timeoutMs > 0
        ? attempt.pipe(
            timeout(millis(timeoutMs)),
            catchTag("TimeoutException", () =>
              fail(
                new ModelError({
                  status: 408,
                  message: `Decisions request timed out after ${timeoutMs}ms`,
                })
              )
            )
          )
        : attempt;
    return retrySalted(timed, rateLimitRetrySchedule(config.retry ?? {}));
  };

  return { choose };
}
