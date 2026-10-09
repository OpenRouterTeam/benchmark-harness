import type { HttpClient, HttpClientError } from "@effect/platform";
import { HttpClientRequest } from "@effect/platform";
import type * as HttpClientResponse from "@effect/platform/HttpClientResponse";
import type { Effect } from "effect/Effect";
import { flatMap, gen, mapError } from "effect/Effect";

import type {
  ReasoningEffort,
  SwitchyardAlgorithm,
} from "../../harness/constants";
import { stripVariantSuffix } from "../../harness/constants";
import type { ModelOutput, ModelUsage, TaskState } from "../../harness/core";
import { ModelError, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
} from "../../providers/app-identity";
import { buildSwitchyardRouterPlugin } from "../../providers/switchyard-router-plugin";
import type { RetryConfig } from "../../runtime/retry";
import { rateLimitRetrySchedule, retrySalted } from "../../runtime/retry";
import { TOOLCALL_SCHEMA_FUZZ_META } from "../benchmark-meta";
import type { CaseLookup } from "./cases";
import { requestOf } from "./tool-case";

export const TOOLCALL_SCHEMA_FUZZ_SYSTEM_MESSAGE =
  "You are a function-calling assistant. Respond only by calling the provided tool, passing the requested arguments exactly as given.";

const DEFAULT_BASE_URL = "https://openrouter.ai/api/v1";

export interface ToolCallSchemaFuzzSolverOptions {
  readonly model: string;
  readonly apiKey: string;
  readonly reasoningEffort: ReasoningEffort;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly endpointId?: string;
  readonly providerOnly?: readonly string[];
  readonly providerIgnore?: readonly string[];
  readonly allowFallbacks?: boolean;
  readonly switchyardAlgorithm?: SwitchyardAlgorithm;
  readonly models?: readonly string[];
  readonly traceHeaders?: Readonly<Record<string, string>>;
  readonly retry?: RetryConfig;
}

const CompletionResponseSchema = z.object({
  choices: z.array(
    z.object({
      finish_reason: z.string().nullish(),
      message: z.object({
        content: z.string().nullish(),
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
  usage: z
    .object({
      prompt_tokens: z.number(),
      completion_tokens: z.number(),
      total_tokens: z.number(),
      cost: z.number().nullish(),
      completion_tokens_details: z
        .object({ reasoning_tokens: z.number().nullish() })
        .nullish(),
    })
    .nullish(),
});
type CompletionUsage = NonNullable<
  z.infer<typeof CompletionResponseSchema>["usage"]
>;

export function buildRequestBody(
  request: Readonly<Record<string, unknown>>,
  options: ToolCallSchemaFuzzSolverOptions
): Readonly<Record<string, unknown>> {
  const provider = definedValues({
    only: options.providerOnly,
    ignore: options.providerIgnore,
    allow_fallbacks: options.allowFallbacks,
  });
  const plugin = buildSwitchyardRouterPlugin(
    stripVariantSuffix(options.model),
    options.switchyardAlgorithm
  );
  return {
    ...request,
    model: options.model,
    stream: false,
    ...(Object.keys(provider).length > 0 && { provider }),
    ...(options.models !== undefined && { models: options.models }),
    ...(plugin !== undefined && { plugins: [plugin] }),
  };
}

function usageToModelUsage(usage: CompletionUsage): ModelUsage {
  return definedValues({
    inputTokens: usage.prompt_tokens,
    outputTokens: usage.completion_tokens,
    totalTokens: usage.total_tokens,
    reasoningTokens:
      usage.completion_tokens_details?.reasoning_tokens ?? undefined,
    totalCost: usage.cost ?? undefined,
  });
}

function readError(error: HttpClientError.HttpClientError): SolverError {
  return new SolverError({
    message: `OpenRouter response read failed: ${error.message}`,
  });
}

function requestCompletion(
  client: HttpClient.HttpClient,
  options: ToolCallSchemaFuzzSolverOptions,
  requestBody: Readonly<Record<string, unknown>>
): Effect<HttpClientResponse.HttpClientResponse, ModelError | SolverError> {
  const baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
  const request = HttpClientRequest.post(`${baseUrl}/chat/completions`).pipe(
    HttpClientRequest.setHeaders({
      ...options.traceHeaders,
      Authorization: `Bearer ${options.apiKey}`,
      "Content-Type": "application/json",
      "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
      "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
      ...definedValues({
        "x-session-id": options.sessionId,
        "X-OR-Endpoint-Id": options.endpointId,
      }),
    }),
    HttpClientRequest.bodyUnsafeJson(requestBody)
  );
  return client.execute(request).pipe(
    mapError(
      (error) =>
        new SolverError({
          message: `OpenRouter request failed: ${error.message}`,
        })
    ),
    flatMap((response) =>
      gen(function* () {
        if (response.status >= 200 && response.status < 300) {
          return response;
        }
        const text = yield* response.text.pipe(mapError(readError));
        const message = `OpenRouter HTTP ${response.status}: ${text}`;
        if (response.status === 429 || response.status >= 500) {
          return yield* new ModelError({ status: response.status, message });
        }
        return yield* new SolverError({ message });
      })
    )
  );
}

export function toolCallSchemaFuzzSolver(
  client: HttpClient.HttpClient,
  {
    options,
    cases,
  }: {
    readonly options: ToolCallSchemaFuzzSolverOptions;
    readonly cases: CaseLookup;
  }
): SolverService {
  return (state: TaskState) =>
    gen(function* () {
      const entry = cases.get(state.sample.id);
      if (entry === undefined) {
        return yield* new SolverError({
          message: `No toolcall_schema_fuzz case for sample ${state.sample.id}`,
        });
      }
      const requestBody = buildRequestBody(
        {
          ...requestOf(entry, TOOLCALL_SCHEMA_FUZZ_SYSTEM_MESSAGE),
          temperature: TOOLCALL_SCHEMA_FUZZ_META.temperature,
          reasoning_effort: options.reasoningEffort,
        },
        options
      );
      const response = yield* retrySalted(
        requestCompletion(client, options, requestBody),
        rateLimitRetrySchedule(options.retry)
      );
      const body = yield* response.json.pipe(mapError(readError));
      const parsed = parseSchema(CompletionResponseSchema, body);
      if (Either.isLeft(parsed)) {
        return yield* new SolverError({
          message: `OpenRouter response parse error: ${parsed.left.message}`,
        });
      }
      const choice = parsed.right.choices[0];
      if (choice === undefined) {
        return yield* new SolverError({
          message: "OpenRouter response did not include a choice",
        });
      }
      const usage = parsed.right.usage ?? undefined;
      const completion = choice.message.content ?? "";
      const output: ModelOutput = {
        completion,
        message: {
          role: "assistant",
          content: completion,
          toolCalls: choice.message.tool_calls ?? [],
        },
        rawResponse: { finish_reason: choice.finish_reason ?? null },
        ...(usage !== undefined && { usage: usageToModelUsage(usage) }),
      };
      return {
        ...state,
        requestBody,
        output,
        messages: [...state.messages, output.message],
        completed: true,
      };
    });
}
