import type { HttpClient } from "@effect/platform";
import { gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  fail as layerFail,
  effect as layerEffect,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";

import type { Dataset } from "../../harness/dataset";
import { Model } from "../../harness/model";
import type { Scorer } from "../../harness/scorer";
import { Scorer as ScorerTag } from "../../harness/scorer";
import type { Solver } from "../../harness/solver";
import { Solver as SolverTag } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import type { RetryConfig } from "../../runtime/retry";
import type {
  BenchmarkRunConfig,
  DecisionsBenchmarkConfig,
} from "../benchmark-config";
import { DECISIONS_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { DecisionsClient, makeDecisionsClientLayer } from "./client";
import { DEFAULT_DATASET_SELECTION, makeDecisionDatasetLayer } from "./dataset";
import { decisionRunLevelScores } from "./run-level";
import { DECISIONS_BENCHMARK_ID, DecisionArm } from "./schema";
import { decisionScorer } from "./scorer";
import { decisionSolver, LLM_ARM_TEMPERATURE, llmSolver } from "./solver";

export function isDecisionsConfig(
  config: BenchmarkRunConfig
): config is DecisionsBenchmarkConfig {
  return config.benchmarkId === DECISIONS_BENCHMARK_ID;
}

function makeSolverLayer(
  input: BenchmarkRunInput,
  config: DecisionsBenchmarkConfig
): Layer<Solver, Error, HttpClient.HttpClient> {
  switch (config.arm) {
    case DecisionArm.Decision: {
      const clientLayer = makeDecisionsClientLayer(
        definedValues({
          apiKey: input.apiKey,
          baseUrl: input.baseUrl,
          sessionId: input.sessionId,
          retry: input.modelRetry,
          traceHeaders: input.traceHeaders,
          timeoutMs: config.timeoutMs,
        })
      );
      return layerEffect(SolverTag)(
        gen(function* () {
          const client = yield* DecisionsClient;
          return SolverTag.of(
            decisionSolver(
              client,
              definedValues({
                model: config.model,
                endpointId: config.endpointId,
                providerOnly: config.providerOnly,
                providerIgnore: config.providerIgnore,
                allowFallbacks: config.allowFallbacks,
              })
            )
          );
        })
      ).pipe(layerProvide(clientLayer));
    }
    case DecisionArm.Llm: {
      const modelLayer =
        input.modelLayer ??
        makeOpenRouterModelLayer(
          definedValues({
            model: config.model,
            models: config.models,
            apiKey: input.apiKey,
            baseUrl: input.baseUrl,
            sessionId: input.sessionId,
            retry: input.modelRetry,
            traceHeaders: input.traceHeaders,
          })
        );
      return layerEffect(SolverTag)(
        gen(function* () {
          const model = yield* Model;
          return SolverTag.of(
            llmSolver(
              model,
              definedValues({
                model: config.model,
                endpointId: config.endpointId,
                inference: {
                  reasoningEffort: config.reasoningEffort,
                  timeoutMs: config.timeoutMs,
                  sort: config.sort,
                  providerOnly: config.providerOnly,
                  providerIgnore: config.providerIgnore,
                  allowFallbacks: config.allowFallbacks,
                  cloudflareVersion: config.cloudflareVersion,
                  costTier: config.costTier,
                  costQualityTradeoff: config.costQualityTradeoff,
                },
              })
            )
          );
        })
      ).pipe(layerProvide(modelLayer));
    }
    default: {
      return config.arm satisfies never;
    }
  }
}

function makeLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (!isDecisionsConfig(benchmarkConfig)) {
    return layerFail(
      new Error(`${DECISIONS_BENCHMARK_ID} received mismatched benchmarkConfig`)
    );
  }
  const datasetLayer = makeDecisionDatasetLayer(
    {
      task: benchmarkConfig.task,
      variant: benchmarkConfig.variant,
      language: benchmarkConfig.language,
    },
    input.datasetRetry
  );
  const scorerLayer = layerSucceed(ScorerTag, ScorerTag.of(decisionScorer));
  return layerMergeAll(
    datasetLayer,
    makeSolverLayer(input, benchmarkConfig),
    scorerLayer
  );
}

export const DECISIONS_BENCHMARK: Benchmark = {
  id: DECISIONS_BENCHMARK_ID,
  makeDatasetLayer: (retry?: RetryConfig) =>
    makeDecisionDatasetLayer(DEFAULT_DATASET_SELECTION, retry),
  temperature: LLM_ARM_TEMPERATURE,
  defaultEpochs: DECISIONS_META.defaultEpochs,
  makeLayer,
  runLevelScores: decisionRunLevelScores,
};
