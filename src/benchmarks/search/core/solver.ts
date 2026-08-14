import type { ResponsesRequest, StreamEvents } from "@openrouter/sdk/models";
import type { Effect } from "effect/Effect";
import {
  catchTag,
  fail,
  flatMap,
  gen,
  map,
  mapError,
  succeed,
  suspend,
  timeoutFail,
} from "effect/Effect";

import type { CostTier, ReasoningEffort } from "../../../harness/constants";
import type { ResponseItem, TaskState } from "../../../harness/core";
import { MessageRole, ModelError } from "../../../harness/core";
import type { ProgressReporterService } from "../../../harness/progress";
import { ProgressReporter } from "../../../harness/progress";
import type { SolverService } from "../../../harness/solver";
import { runHarnessSync } from "../../../internal/effect-logger";
import type { ProviderSort } from "../../../internal/enums";
import { isRecord } from "../../../internal/guards";
import type {
  ResponsesResult,
  ResponsesSendOptions,
  ResponsesService,
} from "../../../providers/responses-client";
import {
  extractCitations,
  toModelError,
  usageFromResponses,
} from "../../../providers/responses-client";
import type { RetryConfig } from "../../../runtime/retry";
import { rateLimitRetrySchedule, retrySalted } from "../../../runtime/retry";
import type { SearchLaneConfig } from "./config";
import { makeSearchProgressTracker } from "./progress";
import { buildSearchRequestBody } from "./request";
import { mergeModelUsages } from "./usage";

export const DEFAULT_SEARCH_TIMEOUT_MS = 420000;

const EMPTY_SEARCH_RESPONSE_MESSAGE = "search response had no answer text";

export interface SearchSolverOptions {
  readonly model: string;
  readonly instructions: string;
  readonly lane: SearchLaneConfig;
  readonly timeoutMs?: number;
  readonly maxOutputTokens?: number;
  readonly temperature?: number;
  readonly reasoningEffort?: ReasoningEffort;
  readonly endpointId?: string;
  readonly sort?: ProviderSort;
  readonly providerOrder?: readonly string[];
  readonly providerOnly?: readonly string[];
  readonly allowFallbacks?: boolean;
  readonly versionOverride?: string;
  readonly costQualityTradeoff?: number;
  readonly costTier?: CostTier;
  readonly retry?: RetryConfig;
}

export interface SearchSolverMetadata {
  readonly citations: readonly {
    url: string;
    title: string;
  }[];
  readonly responseStatus: string | null;
  readonly provider: string | null;
  readonly generationId: string | null;
}

export const SEARCH_SOLVER_METADATA_KEY = "search" as const;

export function searchSolver(
  responses: ResponsesService,
  opts: SearchSolverOptions
): SolverService {
  return (state) =>
    gen(function* () {
      const reporter = yield* ProgressReporter;
      const { epoch } = state;
      const sendSort = opts.sort !== undefined && opts.endpointId === undefined;
      const body = buildSearchRequestBody({
        model: opts.model,
        instructions: opts.instructions,
        problem: state.sample.input,
        lane: opts.lane,
        ...(opts.maxOutputTokens !== undefined && {
          maxOutputTokens: opts.maxOutputTokens,
        }),
        ...(opts.temperature !== undefined && {
          temperature: opts.temperature,
        }),
        ...(opts.reasoningEffort !== undefined && {
          reasoningEffort: opts.reasoningEffort,
        }),
        ...(sendSort && { sort: opts.sort }),
        ...(opts.providerOrder !== undefined && {
          providerOrder: opts.providerOrder,
        }),
        ...(opts.providerOnly !== undefined && {
          providerOnly: opts.providerOnly,
        }),
        ...(opts.allowFallbacks !== undefined && {
          allowFallbacks: opts.allowFallbacks,
        }),
        ...(opts.costQualityTradeoff !== undefined && {
          costQualityTradeoff: opts.costQualityTradeoff,
        }),
        ...(opts.costTier !== undefined && { costTier: opts.costTier }),
      });
      const extraHeaders =
        opts.endpointId === undefined && !opts.lane.providerFlags?.length
          ? undefined
          : {
              ...(opts.endpointId !== undefined && {
                "X-OR-Endpoint-Id": opts.endpointId,
              }),
              ...(opts.lane.providerFlags !== undefined &&
                opts.lane.providerFlags.length > 0 && {
                  "X-Provider-Flags": opts.lane.providerFlags.join(","),
                }),
            };
      const sendOptions = (): ResponsesSendOptions => ({
        timeoutMs: opts.timeoutMs ?? DEFAULT_SEARCH_TIMEOUT_MS,
        ...(extraHeaders !== undefined && { extraHeaders }),
        ...(opts.versionOverride !== undefined && {
          versionOverride: opts.versionOverride,
        }),
      });
      const { result, attemptResults } = yield* sendWithRetry({
        responses,
        body,
        options: () => ({
          ...sendOptions(),
          ...(epoch !== undefined && {
            onStreamEvent: makeStreamEventReporter(
              reporter,
              state.sample.id,
              epoch
            ),
          }),
        }),
        retryConfig: opts.retry,
        timeoutMs: opts.timeoutMs ?? DEFAULT_SEARCH_TIMEOUT_MS,
      });
      return completedState({
        state,
        request: body,
        result,
        attemptResults,
        text: result.text.trim(),
      });
    });
}

function sendWithRetry({
  responses,
  body,
  options,
  retryConfig,
  timeoutMs,
}: {
  readonly responses: ResponsesService;
  readonly body: ResponsesRequest;
  readonly options: () => ResponsesSendOptions;
  readonly retryConfig?: RetryConfig;
  readonly timeoutMs: number;
}): Effect<
  {
    readonly result: ResponsesResult;
    readonly attemptResults: readonly ResponsesResult[];
  },
  ModelError
> {
  const blankResults: ResponsesResult[] = [];
  let lastBlankResult: ResponsesResult | undefined;
  let lastBlankError: ModelError | undefined;
  return retrySalted(
    suspend(() => responses.send(body, options())).pipe(
      mapError(toModelError),
      timeoutFail({
        duration: `${timeoutMs} millis`,
        onTimeout: () =>
          new ModelError({
            status: 504,
            message: `search response exceeded the ${timeoutMs}ms wall-clock deadline`,
          }),
      }),
      flatMap((result) => {
        if (result.status !== "completed") {
          return fail(
            new ModelError({
              status: 503,
              message: `search response ended with status ${result.status ?? "unknown"}`,
            })
          );
        }
        if (result.text.trim() === "") {
          blankResults.push(result);
          lastBlankResult = result;
          lastBlankError = new ModelError({
            status: 503,
            message: EMPTY_SEARCH_RESPONSE_MESSAGE,
          });
          return fail(lastBlankError);
        }
        return succeed(result);
      })
    ),
    rateLimitRetrySchedule(retryConfig ?? {})
  ).pipe(
    catchTag("ModelError", (error) =>
      error === lastBlankError && lastBlankResult !== undefined
        ? succeed(lastBlankResult)
        : fail(error)
    ),
    map((result) => ({
      result,
      attemptResults:
        result.text.trim() === "" ? blankResults : [...blankResults, result],
    }))
  );
}

function makeStreamEventReporter(
  reporter: ProgressReporterService,
  sampleId: string,
  epoch: number
): (event: StreamEvents) => void {
  const track = makeSearchProgressTracker();
  return (event) => {
    const step = track(event);
    if (step !== undefined) {
      runHarnessSync(reporter.onAgentStep(step, sampleId, epoch));
    }
  };
}

function completedState({
  state,
  request,
  result,
  attemptResults,
  text,
}: {
  readonly state: TaskState;
  readonly request: ResponsesRequest;
  readonly result: ResponsesResult;
  readonly attemptResults: readonly ResponsesResult[];
  readonly text: string;
}): TaskState {
  const citations = extractCitations(result.output).map(({ url, title }) => ({
    url,
    title,
  }));
  const usage = mergeModelUsages(
    attemptResults.map((attempt) => usageFromResponses(attempt.usage))
  );
  const generationTimeMs = attemptResults.reduce(
    (total, attempt) => total + attempt.generationTimeMs,
    0
  );
  const metadata: SearchSolverMetadata = {
    citations,
    responseStatus: result.status,
    provider: result.provider,
    generationId: result.generationId,
  };
  return {
    sample: {
      ...state.sample,
      metadata: {
        ...state.sample.metadata,
        [SEARCH_SOLVER_METADATA_KEY]: metadata,
      },
    },
    messages: [
      { role: MessageRole.User, content: state.sample.input },
      ...(text !== ""
        ? [{ role: MessageRole.Assistant, content: text } as const]
        : []),
    ],
    responseItems: responseItemsForCall(request, result),
    requestBody: { ...request },
    output: {
      completion: text,
      message: { role: MessageRole.Assistant, content: text },
      ...(usage !== undefined && { usage }),
      ...(generationTimeMs > 0 && {
        generationTimeMs,
      }),
    },
    completed: true,
    ...(state.epoch !== undefined && { epoch: state.epoch }),
  };
}

function responseItemsForCall(
  request: ResponsesRequest,
  result: ResponsesResult
): readonly ResponseItem[] {
  const inputItems = Array.isArray(request.input)
    ? request.input.filter(isRecord)
    : [];
  return [...inputItems, ...result.output];
}
