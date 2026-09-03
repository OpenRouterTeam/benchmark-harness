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
import { definedValues } from "../../internal/guards";
import { getOriHarness } from "../agent-cli/harness";
import { TERMINAL_BENCH_4_META } from "../benchmark-meta";
import { makeModalSandboxLayer } from "../harbor/modal-sandbox";
import { SandboxSession } from "../harbor/sandbox";
import { terminalBenchScorer } from "../terminal-bench/scorer";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeTerminalBench4DatasetLayer } from "./dataset";
import type { TerminalBench4SolverOpts } from "./solver";
import { terminalBench4Solver } from "./solver";

export const TERMINAL_BENCH_4_ID = TERMINAL_BENCH_4_META.id;

const TERMINAL_BENCH_4_APP_NAME = "openrouter-terminal-bench-4" as const;

function makeTerminalBench4Layer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== "terminal_bench_4") {
    return layerFail(
      new Error("terminal_bench_4 received mismatched benchmarkConfig")
    );
  }
  const { agent } = benchmarkConfig;
  const solverOpts: TerminalBench4SolverOpts = definedValues({
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
    imageRepo: benchmarkConfig.imageRepo,
  });
  const datasetLayer = makeTerminalBench4DatasetLayer(
    definedValues({
      taskSubset: benchmarkConfig.taskSubset,
      maxAgentTimeoutSec: benchmarkConfig.maxAgentTimeoutSec,
    })
  );
  const sandboxLayer: Layer<SandboxSession> = makeModalSandboxLayer({
    appName: TERMINAL_BENCH_4_APP_NAME,
    environment: benchmarkConfig.modalEnv,
  });
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        terminalBench4Solver(sessionFactory, solverOpts, getOriHarness(agent))
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

export const TERMINAL_BENCH_4_BENCHMARK: Benchmark = {
  id: TERMINAL_BENCH_4_ID,
  makeDatasetLayer: () => makeTerminalBench4DatasetLayer(),
  temperature: 0,
  defaultEpochs: TERMINAL_BENCH_4_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeTerminalBench4Layer,
};
