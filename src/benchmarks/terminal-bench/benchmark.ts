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
import { getOriHarness } from "../agent-cli/harness";
import { TERMINAL_BENCH_META } from "../benchmark-meta";
import { makeModalSandboxLayer } from "../harbor/modal-sandbox";
import { SandboxSession } from "../harbor/sandbox";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeTerminalBenchDatasetLayer } from "./dataset";
import type { OriSolverOpts } from "./ori-solver";
import { oriSolver } from "./ori-solver";
import type { OriReasoningEffort, PiThinkingLevel } from "./schema";
import { terminalBenchScorer } from "./scorer";

export const TERMINAL_BENCH_ID = TERMINAL_BENCH_META.id;

const TERMINAL_BENCH_APP_NAME = "openrouter-terminal-bench" as const;

function makeTerminalBenchLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== "terminal_bench") {
    return layerFail(
      new Error("terminal_bench received mismatched benchmarkConfig")
    );
  }
  const { agent } = benchmarkConfig;
  const legacyAgentPackage =
    benchmarkConfig.agentPackage ??
    (agent === "pi" ? benchmarkConfig.piPackage : undefined);
  const effectiveReasoningEffort =
    benchmarkConfig.thinking !== undefined
      ? legacyThinkingToReasoningEffort(benchmarkConfig.thinking)
      : benchmarkConfig.agentReasoningEffort;
  const oriSolverOpts: OriSolverOpts = {
    model: benchmarkConfig.model,
    apiKey: input.apiKey,
    sessionId: input.sessionId,
    ...(benchmarkConfig.endpointId !== undefined && {
      endpointId: benchmarkConfig.endpointId,
    }),
    ...(legacyAgentPackage !== undefined && {
      agentPackage: legacyAgentPackage,
    }),
    oriInstallUrl: benchmarkConfig.oriInstallUrl,
    ...(benchmarkConfig.appendSystemPrompt !== undefined && {
      appendSystemPrompt: benchmarkConfig.appendSystemPrompt,
    }),
    ...(benchmarkConfig.systemPrompt !== undefined && {
      systemPrompt: benchmarkConfig.systemPrompt,
    }),
    agentReasoningEffort: effectiveReasoningEffort,
    oriChannel: benchmarkConfig.oriChannel,
    ...(benchmarkConfig.allowedTools !== undefined && {
      allowedTools: benchmarkConfig.allowedTools,
    }),
    ...(benchmarkConfig.disallowedTools !== undefined && {
      disallowedTools: benchmarkConfig.disallowedTools,
    }),
    isolateAgentConfig: benchmarkConfig.isolateAgentConfig,
  };
  const datasetLayer = makeTerminalBenchDatasetLayer({
    ...(benchmarkConfig.taskSubset !== undefined && {
      taskSubset: benchmarkConfig.taskSubset,
    }),
    ...(benchmarkConfig.maxAgentTimeoutSec !== undefined && {
      maxAgentTimeoutSec: benchmarkConfig.maxAgentTimeoutSec,
    }),
  });
  const sandboxLayer: Layer<SandboxSession> = makeModalSandboxLayer({
    appName: TERMINAL_BENCH_APP_NAME,
    environment: benchmarkConfig.modalEnv,
  });
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        oriSolver(sessionFactory, oriSolverOpts, getOriHarness(agent))
      );
    })
  );
  const scorerLayer = layerSucceed(Scorer, Scorer.of(terminalBenchScorer));
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(sandboxLayer)),
    scorerLayer
  );
}

export const TERMINAL_BENCH_BENCHMARK: Benchmark = {
  id: TERMINAL_BENCH_ID,
  makeDatasetLayer: () => makeTerminalBenchDatasetLayer(),
  temperature: 0,
  defaultEpochs: TERMINAL_BENCH_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeTerminalBenchLayer,
};

function legacyThinkingToReasoningEffort(
  thinking: PiThinkingLevel
): OriReasoningEffort {
  return thinking === "off" ? "none" : thinking;
}
