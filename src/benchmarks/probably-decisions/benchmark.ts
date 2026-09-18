import type { HttpClient } from "@effect/platform";
import { gen, provide as effectProvide } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  fail as layerFail,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";

import type { Dataset } from "../../harness/dataset";
import type { GenerateConfig } from "../../harness/model";
import { Model } from "../../harness/model";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import type { RetryConfig } from "../../runtime/retry";
import type { ProbablyDecisionsConfig } from "../benchmark-config";
import { PROBABLY_DECISIONS_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeDecisionDatasetLayer } from "./dataset";
import { BUNDLED_DATASET_URL, PROBABLY_DECISIONS_ID } from "./schema";
import {
  decisionPrimaryScore,
  decisionRunLevelScores,
  decisionScorer,
} from "./scorer";
import { makeDecisionSolver } from "./solver";

export const PROBABLY_DECISIONS_TEMPERATURE = 0;

export function decisionInferenceConfig(
  config: ProbablyDecisionsConfig
): GenerateConfig {
  return {
    temperature: PROBABLY_DECISIONS_TEMPERATURE,
    ...definedValues({
      endpointId: config.endpointId,
      timeoutMs: config.timeoutMs,
      sort: config.sort,
      providerOnly: config.providerOnly,
      providerIgnore: config.providerIgnore,
      allowFallbacks: config.allowFallbacks,
      cloudflareVersion: config.cloudflareVersion,
      costTier: config.costTier,
      costQualityTradeoff: config.costQualityTradeoff,
      pinModel: config.pinModel,
    }),
    reasoningEffort: config.reasoningEffort,
  };
}

function makeDatasetLayerForConfig(
  config: ProbablyDecisionsConfig,
  retry?: RetryConfig
): Layer<Dataset> {
  return makeDecisionDatasetLayer(
    definedValues({ datasetUrl: config.datasetUrl, retry })
  );
}

function makeLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== PROBABLY_DECISIONS_ID) {
    return layerFail(
      new Error(`${PROBABLY_DECISIONS_ID} received mismatched benchmarkConfig`)
    );
  }
  const modelLayer =
    input.modelLayer ??
    makeOpenRouterModelLayer(
      definedValues({
        model: benchmarkConfig.model,
        apiKey: input.apiKey,
        baseUrl: input.baseUrl,
        sessionId: input.sessionId,
        retry: input.modelRetry,
        traceHeaders: input.traceHeaders,
      })
    );
  const judgeLayer =
    benchmarkConfig.judgeModel === undefined
      ? modelLayer
      : makeOpenRouterModelLayer(
          definedValues({
            model: benchmarkConfig.judgeModel,
            apiKey: input.apiKey,
            baseUrl: input.baseUrl,
            sessionId: input.sessionId,
            retry: input.modelRetry,
            traceHeaders: input.traceHeaders,
          })
        );
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* Model;
      const judge = yield* Model.pipe(effectProvide(judgeLayer));
      return Solver.of(
        makeDecisionSolver(model, {
          judge,
          mode: benchmarkConfig.mode,
          program: benchmarkConfig.program,
          maxResearchSteps: benchmarkConfig.maxResearchSteps,
          inference: decisionInferenceConfig(benchmarkConfig),
        })
      );
    })
  ).pipe(layerProvide(modelLayer));
  return layerMergeAll(
    makeDatasetLayerForConfig(benchmarkConfig, input.datasetRetry),
    solverLayer,
    layerSucceed(Scorer, Scorer.of(decisionScorer))
  );
}

export const PROBABLY_DECISIONS_BENCHMARK: Benchmark = {
  id: PROBABLY_DECISIONS_META.id,
  makeDatasetLayer: () =>
    makeDecisionDatasetLayer({ datasetUrl: BUNDLED_DATASET_URL }),
  makeLayer,
  temperature: PROBABLY_DECISIONS_TEMPERATURE,
  defaultEpochs: PROBABLY_DECISIONS_META.defaultEpochs,
  degradeSolverErrors: true,
  runLevelScores: decisionRunLevelScores,
  primaryScore: decisionPrimaryScore,
};
