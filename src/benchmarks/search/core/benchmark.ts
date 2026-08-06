import type { HttpClient } from "@effect/platform";
import { gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect, fail, mergeAll, provide, succeed } from "effect/Layer";

import type { Dataset } from "../../../harness/dataset";
import type { ScorerService } from "../../../harness/scorer";
import { Scorer } from "../../../harness/scorer";
import type { SolverService } from "../../../harness/solver";
import { Solver } from "../../../harness/solver";
import type { ResponsesService } from "../../../providers/responses-client";
import {
  makeResponsesLayer,
  Responses,
} from "../../../providers/responses-client";
import type { RetryConfig } from "../../../runtime/retry";
import type { SearchBenchmarkConfig } from "../../benchmark-config";
import { isSearchBenchmarkConfig } from "../../benchmark-config";
import type { BenchmarkRunInput } from "../../types";
import type { SearchSolverOptions } from "./solver";

export const DEFAULT_SEARCH_MAX_OUTPUT_TOKENS = 128000;

interface SearchBenchmarkLayerDefinition {
  readonly benchmarkId: string;
  readonly instructions: string;
  readonly temperature: number;
  readonly makeDatasetLayer: (retry?: RetryConfig) => Layer<Dataset>;
  readonly makeSolver: (
    responses: ResponsesService,
    options: SearchSolverOptions
  ) => SolverService;
  readonly scorer: ScorerService;
  readonly maxOutputTokens?: number;
}

export function searchSolverOptionsFromConfig({
  config,
  instructions,
  temperature,
  retry,
  maxOutputTokens = DEFAULT_SEARCH_MAX_OUTPUT_TOKENS,
  maxOutputTokensCeiling,
}: {
  readonly config: SearchBenchmarkConfig;
  readonly instructions: string;
  readonly temperature: number;
  readonly retry?: RetryConfig;
  readonly maxOutputTokens?: number;
  readonly maxOutputTokensCeiling?: number;
}): SearchSolverOptions {
  const requestedMaxOutputTokens = config.maxTokens ?? maxOutputTokens;
  return {
    model: config.model,
    instructions,
    lane: config.lane,
    maxOutputTokens:
      maxOutputTokensCeiling === undefined
        ? requestedMaxOutputTokens
        : Math.min(requestedMaxOutputTokens, maxOutputTokensCeiling),
    temperature: config.temperature ?? temperature,
    ...(config.timeoutMs !== undefined && { timeoutMs: config.timeoutMs }),
    ...(config.reasoningEffort !== undefined && {
      reasoningEffort: config.reasoningEffort,
    }),
    ...(config.endpointId !== undefined && { endpointId: config.endpointId }),
    ...(config.sort !== undefined && { sort: config.sort }),
    ...(config.providerOrder !== undefined && {
      providerOrder: config.providerOrder,
    }),
    ...(config.providerOnly !== undefined && {
      providerOnly: config.providerOnly,
    }),
    ...(config.allowFallbacks !== undefined && {
      allowFallbacks: config.allowFallbacks,
    }),
    ...(config.cloudflareVersion !== undefined && {
      versionOverride: config.cloudflareVersion,
    }),
    ...(config.costQualityTradeoff !== undefined && {
      costQualityTradeoff: config.costQualityTradeoff,
    }),
    ...(config.costTier !== undefined && { costTier: config.costTier }),
    ...(retry !== undefined && { retry }),
  };
}

export function makeSearchBenchmarkLayer(
  input: BenchmarkRunInput,
  definition: SearchBenchmarkLayerDefinition
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const config = input.benchmarkConfig;
  if (
    !isSearchBenchmarkConfig(config) ||
    config.benchmarkId !== definition.benchmarkId
  ) {
    return fail(
      new Error(`${definition.benchmarkId} received mismatched benchmarkConfig`)
    );
  }
  const responsesLayer = makeResponsesLayer({
    apiKey: input.apiKey,
    ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
    sessionId: input.sessionId,
  });
  const options = searchSolverOptionsFromConfig({
    config,
    instructions: definition.instructions,
    temperature: definition.temperature,
    retry: input.modelRetry,
    maxOutputTokens: definition.maxOutputTokens,
    ...(input.maxOutputTokensCeiling !== undefined && {
      maxOutputTokensCeiling: input.maxOutputTokensCeiling,
    }),
  });
  const solverLayer = effect(Solver)(
    gen(function* () {
      const responses = yield* Responses;
      return Solver.of(definition.makeSolver(responses, options));
    })
  );
  return mergeAll(
    definition.makeDatasetLayer(input.datasetRetry),
    solverLayer.pipe(provide(responsesLayer)),
    succeed(Scorer, Scorer.of(definition.scorer))
  );
}
