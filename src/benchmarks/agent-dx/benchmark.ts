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
import { Either } from "../../internal/either";
import type { BenchmarkRunConfig } from "../benchmark-config";
import { AGENT_DX_META } from "../benchmark-meta";
import { makeModalSandboxLayer } from "../terminal-bench/modal-sandbox";
import { SandboxSession } from "../terminal-bench/sandbox";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { agentDxTasksDir, loadTask, makeAgentDxDatasetLayer } from "./dataset";
import { agentDxScorer } from "./scorer";
import type { AgentDxSolverOpts } from "./solver";
import { harnessSolver } from "./solver";

export const AGENT_DX_ID = AGENT_DX_META.id;

function makeAgentDxLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== "agent_dx") {
    return layerFail(new Error("agent_dx received mismatched benchmarkConfig"));
  }

  if (benchmarkConfig.endpointId !== undefined) {
    return layerFail(
      new Error(
        "benchmark config: agent_dx does not support endpointId (endpoint pinning)"
      )
    );
  }

  const sandboxKeyError = invalidSandboxKeyPairing(benchmarkConfig);
  if (sandboxKeyError !== undefined) {
    return layerFail(new Error(sandboxKeyError));
  }

  const solverOpts: AgentDxSolverOpts = {
    model: benchmarkConfig.model,
    apiKey: input.apiKey,
    ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
    profile: benchmarkConfig.profile,
    opencodePackage: benchmarkConfig.opencodePackage,
    skillsSource: benchmarkConfig.skillsSource,
    docsSource: benchmarkConfig.docsSource,
    ...(benchmarkConfig.docsAddendum !== undefined && {
      docsAddendum: benchmarkConfig.docsAddendum,
    }),
    ...(benchmarkConfig.mcpAddendum !== undefined && {
      mcpAddendum: benchmarkConfig.mcpAddendum,
    }),
    ...(benchmarkConfig.judgeModel !== undefined && {
      judgeModel: benchmarkConfig.judgeModel,
    }),
    sandboxKey: benchmarkConfig.sandboxKey,
  };

  const datasetLayer = makeConfiguredDatasetLayer(benchmarkConfig);

  const sandboxLayer: Layer<SandboxSession> = makeModalSandboxLayer({
    environment: benchmarkConfig.modalEnv,
  });

  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        harnessSolver(sessionFactory, solverOpts, benchmarkConfig.harness)
      );
    })
  );

  const scorerLayer = layerSucceed(Scorer, Scorer.of(agentDxScorer));

  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(sandboxLayer)),
    scorerLayer
  );
}

export function invalidSandboxKeyPairing(
  config: {
    readonly sandboxKey: "provided" | "absent";
    readonly suite: string;
    readonly taskSubset?: readonly string[];
  },
  tasksDir: string = agentDxTasksDir()
): string | undefined {
  if (config.sandboxKey !== "absent") {
    return undefined;
  }
  if (config.suite !== "discoverability") {
    return `benchmark config: sandboxKey "absent" requires suite "discoverability" (got "${config.suite}")`;
  }
  const offSuite = (config.taskSubset ?? []).filter((id) => {
    const loaded = Either.try(() => loadTask(id, tasksDir));
    return (
      Either.isRight(loaded) &&
      loaded.right.taskToml.task.suite !== "discoverability"
    );
  });
  if (offSuite.length > 0) {
    return `benchmark config: sandboxKey "absent" requires every taskSubset task to be in the discoverability suite (got: ${offSuite.join(", ")})`;
  }
  return undefined;
}

function makeConfiguredDatasetLayer(
  benchmarkConfig: BenchmarkRunConfig & { readonly benchmarkId: "agent_dx" }
): Layer<Dataset> {
  return makeAgentDxDatasetLayer({
    suite: benchmarkConfig.suite,
    ...(benchmarkConfig.taskSubset !== undefined && {
      taskSubset: benchmarkConfig.taskSubset,
    }),
    ...(benchmarkConfig.maxAgentTimeoutSec !== undefined && {
      maxAgentTimeoutSec: benchmarkConfig.maxAgentTimeoutSec,
    }),
  });
}

export const AGENT_DX_BENCHMARK: Benchmark = {
  id: AGENT_DX_ID,
  makeDatasetLayer: () => makeAgentDxDatasetLayer(),
  makeDatasetLayerForConfig: (config) =>
    config.benchmarkId === "agent_dx"
      ? makeConfiguredDatasetLayer(config)
      : makeAgentDxDatasetLayer(),
  temperature: 0,
  defaultEpochs: AGENT_DX_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeAgentDxLayer,
};
