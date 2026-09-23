import { HttpClient } from "@effect/platform";
import type { Effect } from "effect/Effect";
import { flatMap, gen, map, provideService, succeed } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  fail as layerFail,
  effect as layerEffect,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";

import type { Dataset } from "../../harness/dataset";
import type { GenerateConfig } from "../../harness/model";
import { Model } from "../../harness/model";
import { Scorer } from "../../harness/scorer";
import type { SolverService } from "../../harness/solver";
import { Solver } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import { makeModalSandboxLayer } from "../../sandbox/modal";
import { resolveModalRegions } from "../../sandbox/modal-regions";
import { SandboxSession } from "../../sandbox/session";
import { sandboxAgentCandidateModelsError } from "../agent-cli/candidate-models";
import { getOriHarness } from "../agent-cli/harness";
import type { AgentCliOpts } from "../agent-cli/runner";
import type {
  BenchmarkRunConfig,
  RecoveryBenchConfig,
} from "../benchmark-config";
import { RECOVERY_BENCH_META } from "../benchmark-meta";
import { terminalBenchScorer } from "../terminal-bench/scorer";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeRecoveryBenchDatasetLayer } from "./dataset";
import type { RecoveryBenchSampleMeta } from "./dataset";
import type { RecoveryBenchSolverOpts, Summarizer } from "./solver";
import { recoveryBenchSolver } from "./solver";
import { makeModelSummarizer } from "./summarizer";
import { fetchTrajectoryText } from "./trajectory-source";

export const RECOVERY_BENCH_ID = RECOVERY_BENCH_META.id;

const RECOVERY_BENCH_APP_NAME = "openrouter-recovery-bench" as const;

function makeConfiguredDatasetLayer(
  benchmarkConfig: RecoveryBenchConfig
): Layer<Dataset> {
  return makeRecoveryBenchDatasetLayer(
    definedValues({
      taskSubset: benchmarkConfig.taskSubset,
      maxAgentTimeoutSec: benchmarkConfig.maxAgentTimeoutSec,
      maxInstructionBytes: benchmarkConfig.maxInstructionBytes,
    })
  );
}

function mismatchedConfigLayer<Out>(): Layer<Out, Error> {
  return layerFail(
    new Error("recovery_bench received mismatched benchmarkConfig")
  );
}

function isRecoveryBenchConfig(
  config: BenchmarkRunConfig
): config is RecoveryBenchConfig {
  return config.benchmarkId === "recovery_bench";
}

function makeRecoveryBenchDatasetLayerForConfig(
  config: BenchmarkRunConfig
): Layer<Dataset, Error> {
  if (!isRecoveryBenchConfig(config)) {
    return mismatchedConfigLayer();
  }
  return makeConfiguredDatasetLayer(config);
}

function makeRecoveryBenchLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (!isRecoveryBenchConfig(benchmarkConfig)) {
    return mismatchedConfigLayer();
  }
  const { agent } = benchmarkConfig;
  const candidateModelsError = sandboxAgentCandidateModelsError({
    benchmarkId: benchmarkConfig.benchmarkId,
    agent,
    models: benchmarkConfig.models,
  });
  if (candidateModelsError !== undefined) {
    return layerFail(candidateModelsError);
  }
  const agentCli: AgentCliOpts = definedValues({
    model: benchmarkConfig.model,
    apiKey: input.apiKey,
    sessionId: input.sessionId,
    endpointId: benchmarkConfig.endpointId,
    agentPackage: benchmarkConfig.agentPackage,
    oriInstallUrl: benchmarkConfig.oriInstallUrl,
    appendSystemPrompt: benchmarkConfig.appendSystemPrompt,
    systemPrompt: benchmarkConfig.systemPrompt,
    agentReasoningEffort: benchmarkConfig.agentReasoningEffort,
    oriChannel: benchmarkConfig.oriChannel,
    allowedTools: benchmarkConfig.allowedTools,
    disallowedTools: benchmarkConfig.disallowedTools,
    isolateAgentConfig: benchmarkConfig.isolateAgentConfig,
  });
  const datasetLayer = makeConfiguredDatasetLayer(benchmarkConfig);
  const sandboxLayer: Layer<SandboxSession> = makeModalSandboxLayer({
    appName: RECOVERY_BENCH_APP_NAME,
    environment: benchmarkConfig.modalEnv,
    regions: resolveModalRegions(
      benchmarkConfig.model,
      benchmarkConfig.modalRegions
    ),
  });
  const summaryModel = benchmarkConfig.summaryModel;
  const summaryModelLayer:
    | Layer<Model, Error, HttpClient.HttpClient>
    | undefined =
    benchmarkConfig.messageMode === "summary" && summaryModel !== undefined
      ? (input.modelLayer ??
        makeOpenRouterModelLayer(
          definedValues({
            model: summaryModel,
            apiKey: input.apiKey,
            baseUrl: input.baseUrl,
            sessionId: input.sessionId,
            retry: input.modelRetry,
            traceHeaders: input.traceHeaders,
          })
        ))
      : undefined;
  const summaryConfig: GenerateConfig = definedValues({
    temperature: benchmarkConfig.temperature,
    reasoningEffort: benchmarkConfig.reasoningEffort,
    timeoutMs: benchmarkConfig.timeoutMs,
    providerOnly: benchmarkConfig.providerOnly,
    providerIgnore: benchmarkConfig.providerIgnore,
    allowFallbacks: benchmarkConfig.allowFallbacks,
  });
  const summarizerEffect: Effect<Summarizer | undefined, never, Model> =
    summaryModelLayer === undefined
      ? succeed(undefined)
      : map(Model, (model) => makeModelSummarizer(model, summaryConfig));
  const makeSolver = (
    summarize: Summarizer | undefined
  ): Effect<SolverService, never, SandboxSession | HttpClient.HttpClient> =>
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      const httpClient = yield* HttpClient.HttpClient;
      const opts: RecoveryBenchSolverOpts = definedValues({
        agentCli,
        messageMode: benchmarkConfig.messageMode,
        replayCommandTimeoutSec: benchmarkConfig.replayCommandTimeoutSec,
        maxInstructionBytes: benchmarkConfig.maxInstructionBytes,
        fetchTrajectory: (meta: RecoveryBenchSampleMeta) =>
          fetchTrajectoryText(meta, input.modelRetry).pipe(
            provideService(HttpClient.HttpClient, httpClient)
          ),
        summarize,
      });
      return recoveryBenchSolver(sessionFactory, opts, getOriHarness(agent));
    });
  const solverLayer =
    summaryModelLayer === undefined
      ? layerEffect(Solver)(makeSolver(undefined).pipe(map(Solver.of))).pipe(
          layerProvide(sandboxLayer)
        )
      : layerEffect(Solver)(
          summarizerEffect.pipe(flatMap(makeSolver), map(Solver.of))
        ).pipe(layerProvide(layerMergeAll(sandboxLayer, summaryModelLayer)));
  const scorerLayer = layerSucceed(Scorer, Scorer.of(terminalBenchScorer));
  return layerMergeAll(datasetLayer, solverLayer, scorerLayer);
}

export const RECOVERY_BENCH_BENCHMARK: Benchmark = {
  id: RECOVERY_BENCH_ID,
  makeDatasetLayer: () => makeRecoveryBenchDatasetLayer(),
  makeDatasetLayerForConfig: makeRecoveryBenchDatasetLayerForConfig,
  temperature: 0,
  defaultEpochs: RECOVERY_BENCH_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeRecoveryBenchLayer,
};
