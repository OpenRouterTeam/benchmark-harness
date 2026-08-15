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
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import {
  makeResponsesModelLayer,
  ResponsesModel,
} from "../../providers/responses-model";
import {
  SWE_ATLAS_QA_META,
  SWE_ATLAS_RF_META,
  SWE_ATLAS_TW_META,
} from "../benchmark-meta";
import { makeModalSandboxLayer } from "../harbor/modal-sandbox";
import { SandboxSession } from "../harbor/sandbox";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeSweAtlasDatasetLayer, SWE_ATLAS_DATASET_IDS } from "./dataset";
import type { SweAtlasTrack } from "./schema";
import { sweAtlasScorer } from "./scorer";
import { makeSweAtlasSolver } from "./solver";

const SWE_ATLAS_TEMPERATURE = 0;

function makeSweAtlasLayer(
  track: SweAtlasTrack,
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const expectedId = SWE_ATLAS_DATASET_IDS[track];
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== expectedId) {
    return layerFail(
      new Error(`${expectedId} received mismatched benchmarkConfig`)
    );
  }
  const datasetLayer = makeSweAtlasDatasetLayer({
    track,
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
  const sandboxLayer = makeModalSandboxLayer({
    appName: "openrouter-swe-atlas",
    environment: benchmarkConfig.modalEnv,
  });
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* ResponsesModel;
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        makeSweAtlasSolver(model, sessionFactory, {
          track,
          model: benchmarkConfig.model,
          apiKey: input.apiKey,
          judgeModel: benchmarkConfig.judgeModel,
          stepLimit: benchmarkConfig.stepLimit,
          ...(benchmarkConfig.endpointId !== undefined && {
            endpointId: benchmarkConfig.endpointId,
          }),
          inference: {
            temperature: benchmarkConfig.temperature,
            maxTokens: benchmarkConfig.maxTokens,
            reasoningEffort: benchmarkConfig.reasoningEffort,
            timeoutMs: benchmarkConfig.timeoutMs,
            sort: benchmarkConfig.sort,
            providerOnly: benchmarkConfig.providerOnly,
            allowFallbacks: benchmarkConfig.allowFallbacks,
            cloudflareVersion: benchmarkConfig.cloudflareVersion,
            costTier: benchmarkConfig.costTier,
            costQualityTradeoff: benchmarkConfig.costQualityTradeoff,
          },
        })
      );
    })
  );
  const scorerLayer = layerSucceed(Scorer, Scorer.of(sweAtlasScorer));
  const infraLayer = layerMergeAll(modelLayer, sandboxLayer);
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(infraLayer)),
    scorerLayer
  );
}

export const SWE_ATLAS_QA_BENCHMARK: Benchmark = {
  id: SWE_ATLAS_QA_META.id,
  makeDatasetLayer: () => makeSweAtlasDatasetLayer({ track: "qa" }),
  temperature: SWE_ATLAS_TEMPERATURE,
  defaultEpochs: SWE_ATLAS_QA_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: (input) => makeSweAtlasLayer("qa", input),
};

export const SWE_ATLAS_TW_BENCHMARK: Benchmark = {
  id: SWE_ATLAS_TW_META.id,
  makeDatasetLayer: () => makeSweAtlasDatasetLayer({ track: "tw" }),
  temperature: SWE_ATLAS_TEMPERATURE,
  defaultEpochs: SWE_ATLAS_TW_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: (input) => makeSweAtlasLayer("tw", input),
};

export const SWE_ATLAS_RF_BENCHMARK: Benchmark = {
  id: SWE_ATLAS_RF_META.id,
  makeDatasetLayer: () => makeSweAtlasDatasetLayer({ track: "rf" }),
  temperature: SWE_ATLAS_TEMPERATURE,
  defaultEpochs: SWE_ATLAS_RF_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: (input) => makeSweAtlasLayer("rf", input),
};
