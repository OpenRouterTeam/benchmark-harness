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
import { DEEP_SWE_META } from "../benchmark-meta";
import { makeModalSandboxLayer } from "../harbor/modal-sandbox";
import { SandboxSession } from "../harbor/sandbox";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { DEEP_SWE_DATASET_ID, makeDeepSweDatasetLayer } from "./dataset";
import { deepSweScorer } from "./scorer";
import { makeDeepSweSolver } from "./solver";

const DEEP_SWE_HARNESS_TEMPERATURE = 0;

function makeDeepSweLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== DEEP_SWE_DATASET_ID) {
    return layerFail(
      new Error(`${DEEP_SWE_DATASET_ID} received mismatched benchmarkConfig`)
    );
  }
  const datasetLayer = makeDeepSweDatasetLayer({
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
    appName: "openrouter-deep-swe",
    environment: benchmarkConfig.modalEnv,
  });
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* ResponsesModel;
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        makeDeepSweSolver(model, sessionFactory, {
          model: benchmarkConfig.model,
          apiKey: input.apiKey,
          stepLimit: benchmarkConfig.stepLimit,
          agent: benchmarkConfig.agent,
          agentCli: {
            model: benchmarkConfig.model,
            apiKey: input.apiKey,
            sessionId: input.sessionId,
            ...(benchmarkConfig.endpointId !== undefined && {
              endpointId: benchmarkConfig.endpointId,
            }),
            ...(benchmarkConfig.agentPackage !== undefined && {
              agentPackage: benchmarkConfig.agentPackage,
            }),
            oriInstallUrl: benchmarkConfig.oriInstallUrl,
            agentReasoningEffort: benchmarkConfig.agentReasoningEffort,
            oriChannel: benchmarkConfig.oriChannel,
            ...(benchmarkConfig.systemPrompt !== undefined && {
              systemPrompt: benchmarkConfig.systemPrompt,
            }),
            ...(benchmarkConfig.appendSystemPrompt !== undefined && {
              appendSystemPrompt: benchmarkConfig.appendSystemPrompt,
            }),
            ...(benchmarkConfig.allowedTools !== undefined && {
              allowedTools: benchmarkConfig.allowedTools,
            }),
            ...(benchmarkConfig.disallowedTools !== undefined && {
              disallowedTools: benchmarkConfig.disallowedTools,
            }),
            isolateAgentConfig: benchmarkConfig.isolateAgentConfig,
          },
          ...(benchmarkConfig.endpointId !== undefined && {
            endpointId: benchmarkConfig.endpointId,
          }),
          sessionId: input.sessionId,
          inference: {
            temperature: benchmarkConfig.temperature,
            maxTokens: benchmarkConfig.maxTokens,
            reasoningEffort: benchmarkConfig.reasoningEffort,
            timeoutMs: benchmarkConfig.timeoutMs,
            sort: benchmarkConfig.sort,
            providerOnly: benchmarkConfig.providerOnly,
            providerIgnore: benchmarkConfig.providerIgnore,
            allowFallbacks: benchmarkConfig.allowFallbacks,
            cloudflareVersion: benchmarkConfig.cloudflareVersion,
            costTier: benchmarkConfig.costTier,
            costQualityTradeoff: benchmarkConfig.costQualityTradeoff,
          },
        })
      );
    })
  );
  const scorerLayer = layerSucceed(Scorer, Scorer.of(deepSweScorer));
  const infraLayer = layerMergeAll(modelLayer, sandboxLayer);
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(infraLayer)),
    scorerLayer
  );
}

export const DEEP_SWE_BENCHMARK: Benchmark = {
  id: DEEP_SWE_META.id,
  makeDatasetLayer: () => makeDeepSweDatasetLayer(),
  temperature: DEEP_SWE_HARNESS_TEMPERATURE,
  defaultEpochs: DEEP_SWE_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeDeepSweLayer,
};
