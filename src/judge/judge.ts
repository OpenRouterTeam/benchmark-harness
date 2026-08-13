import type {
  ResponsesRequest,
  TextExtendedConfig,
} from "@openrouter/sdk/models";
import type { Effect } from "effect/Effect";
import { fail, flatMap, mapError, retry, succeed } from "effect/Effect";

import type { ReasoningEffort } from "../harness/constants";
import type { ModelUsage } from "../harness/core";
import { ModelError } from "../harness/core";
import { Either } from "../internal/either";
import type { ResponsesService } from "../providers/responses-client";
import {
  toModelError,
  usageFromResponses,
} from "../providers/responses-client";
import { responsesMessage } from "../providers/responses-model";
import type { RetryConfig } from "../runtime/retry";
import { rateLimitRetrySchedule } from "../runtime/retry";

export interface JudgeConfig {
  readonly judgeModel: string;
  readonly temperature?: number;
  readonly reasoningEffort?: ReasoningEffort;
  readonly timeoutMs?: number;
  readonly retry?: RetryConfig;
  readonly versionOverride?: string;
}

export const DEFAULT_JUDGE_TIMEOUT_MS = 90000;

export interface JudgeCallSpec<T> {
  readonly instructions?: string;
  readonly userInput: string;
  readonly schemaName: string;
  readonly jsonSchema?: Record<string, unknown>;
  readonly parseVerdict: (text: string) => Either.Either<T, string>;
  readonly parseFailureFallback?: T;
}

export interface JudgeResult<T> {
  readonly verdict: T;
  readonly usage: ModelUsage | undefined;
  readonly parseError?: string;
}

export function judgeCall<T>(
  responses: ResponsesService,
  config: JudgeConfig,
  spec: JudgeCallSpec<T>
): Effect<JudgeResult<T>, ModelError> {
  const text: TextExtendedConfig | undefined =
    spec.jsonSchema === undefined
      ? undefined
      : {
          format: {
            type: "json_schema",
            name: spec.schemaName,
            strict: true,
            schema: spec.jsonSchema,
          },
        };
  const body: ResponsesRequest = {
    model: config.judgeModel,
    input: [responsesMessage("user", spec.userInput)],
    ...(text !== undefined && { text }),
    ...(spec.instructions !== undefined && { instructions: spec.instructions }),
    ...(config.temperature !== undefined && {
      temperature: config.temperature,
    }),
    ...(config.reasoningEffort !== undefined && {
      reasoning: { effort: config.reasoningEffort },
    }),
  };
  const sendOptions = {
    timeoutMs: config.timeoutMs ?? DEFAULT_JUDGE_TIMEOUT_MS,
    ...(config.versionOverride !== undefined && {
      versionOverride: config.versionOverride,
    }),
  };
  return responses.send(body, sendOptions).pipe(
    mapError(toModelError),
    retry(rateLimitRetrySchedule(config.retry ?? {})),
    flatMap((result) => {
      const parsed = spec.parseVerdict(result.text);
      const usage = usageFromResponses(result.usage);
      if (Either.isLeft(parsed)) {
        const parseError = `judge verdict parse failed (${spec.schemaName}): ${parsed.left}`;
        if (spec.parseFailureFallback !== undefined) {
          return succeed({
            verdict: spec.parseFailureFallback,
            usage,
            parseError,
          });
        }
        return fail(
          new ModelError({
            message: parseError,
          })
        );
      }
      return succeed({ verdict: parsed.right, usage });
    })
  );
}
