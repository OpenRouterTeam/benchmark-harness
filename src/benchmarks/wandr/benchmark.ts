import type { HttpClient } from "@effect/platform";
import { gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  fail as layerFail,
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";

import type { Dataset } from "../../harness/dataset";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import {
  makeResponsesModelLayer,
  ResponsesModel,
} from "../../providers/responses-model";
import type { InferenceOverride, WandrConfig } from "../benchmark-config";
import { WANDR_META } from "../benchmark-meta";
import { SandboxSession } from "../harbor/sandbox";
import { makeHarborSandboxLayer } from "../harbor/sandbox-layer";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeWandrDatasetLayer } from "./dataset";
import { WANDR_DATASET_ID } from "./schema";
import { wandrPrimaryScore, wandrRunLevelScores, wandrScorer } from "./scorer";
import { makeWandrSolver } from "./solver";

const WANDR_TEMPERATURE = 0;

export function wandrInferenceOverride(config: WandrConfig): InferenceOverride {
  return {
    temperature: config.temperature,
    maxTokens: config.maxTokens,
    reasoningEffort: config.reasoningEffort,
    timeoutMs: config.timeoutMs,
    sort: config.sort,
    providerOnly: config.providerOnly,
    allowFallbacks: config.allowFallbacks,
    cloudflareVersion: config.cloudflareVersion,
    costTier: config.costTier,
    costQualityTradeoff: config.costQualityTradeoff,
  };
}

function makeWandrLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== WANDR_DATASET_ID) {
    return layerFail(
      new Error(`${WANDR_DATASET_ID} received mismatched benchmarkConfig`)
    );
  }
  const datasetLayer = makeWandrDatasetLayer({
    ...(benchmarkConfig.taskSubset !== undefined && {
      taskSubset: benchmarkConfig.taskSubset,
    }),
    ...(benchmarkConfig.maxAgentTimeoutSec !== undefined && {
      maxAgentTimeoutSec: benchmarkConfig.maxAgentTimeoutSec,
    }),
  });
  const modelLayer =
    input.responsesModelLayer ??
    makeResponsesModelLayer({
      model: benchmarkConfig.model,
      apiKey: input.apiKey,
      ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
      sessionId: input.sessionId,
      ...(input.modelRetry !== undefined && { retry: input.modelRetry }),
    });
  const sandboxLayer = makeHarborSandboxLayer({
    appName: "openrouter-wandr",
    environment: benchmarkConfig.modalEnv,
  });
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* ResponsesModel;
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        makeWandrSolver(model, sessionFactory, {
          apiKey: input.apiKey,
          stepLimit: benchmarkConfig.stepLimit,
          serverTools: benchmarkConfig.serverTools,
          ...(benchmarkConfig.endpointId !== undefined && {
            endpointId: benchmarkConfig.endpointId,
          }),
          sessionId: input.sessionId,
          inference: wandrInferenceOverride(benchmarkConfig),
        })
      );
    })
  ).pipe(layerProvide(layerMergeAll(modelLayer, sandboxLayer)));
  return layerMergeAll(
    datasetLayer,
    solverLayer,
    layerSucceed(Scorer, Scorer.of(wandrScorer))
  );
}

export const WANDR_BENCHMARK: Benchmark = {
  id: WANDR_META.id,
  makeDatasetLayer: () => makeWandrDatasetLayer(),
  makeLayer: makeWandrLayer,
  temperature: WANDR_TEMPERATURE,
  defaultEpochs: WANDR_META.defaultEpochs,
  degradeSolverErrors: true,
  runLevelScores: wandrRunLevelScores,
  primaryScore: wandrPrimaryScore,
};
