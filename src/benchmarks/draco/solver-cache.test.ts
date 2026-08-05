import { describe, expect, it } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ResponsesRequest } from "@openrouter/sdk/models";
import {
  gen,
  provide,
  runPromise,
  succeed as effectSucceed,
} from "effect/Effect";
import {
  effect,
  mergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";

import {
  noopProgressLayer,
  noopCheckpointLayer,
} from "../../../test/helpers/noop-progress-layer";
import { initialTaskState } from "../../harness/core";
import { Solver } from "../../harness/solver";
import { isRecord } from "../../internal/guards";
import type {
  ResponsesResult,
  ResponsesSendOptions,
} from "../../providers/responses-client";
import { Responses } from "../../providers/responses-client";
import {
  ArtifactStoreService,
  makeFsArtifactStoreLayer,
} from "./artifact-store";
import type { Criterion, DracoPanelConfig } from "./schemas";
import { dracoSolver } from "./solver";

function fixtureResponsesLayer(
  onSend: (model: string) => void,
  analysisModels: readonly string[] = []
) {
  const send = (body: ResponsesRequest, _options: ResponsesSendOptions) => {
    const bodyRecord: unknown = body;
    const model =
      isRecord(bodyRecord) && typeof bodyRecord["model"] === "string"
        ? bodyRecord["model"]
        : "?";
    onSend(model);
    const isJudge = model.startsWith("judge/");
    const isFusion = model === "openrouter/fusion";
    const text = isJudge
      ? '{"verdict":"MET","justification":"ok"}'
      : `answer from ${model}`;
    const output: Record<string, unknown>[] = [
      { type: "message", content: [{ type: "output_text", text }] },
    ];
    if (isFusion) {
      output.push({
        type: "openrouter:fusion",
        status: "completed",
        responses: analysisModels.map((m) => ({
          model: m,
          text: `panel from ${m}`,
        })),
      });
    }
    const result: ResponsesResult = {
      id: `gen-${model}-${Math.random()}`,
      model,
      status: "completed",
      output,
      usage: { cost: 0.001 },
      text,
      generationId: null,
      provider: "fixture",
      generationTimeMs: 0,
    };
    return effectSucceed(result);
  };
  return layerSucceed(Responses, Responses.of({ send }));
}

const CRITERION: Criterion = {
  id: "c1",
  section: "Factual",
  sectionId: "factual",
  weight: 10,
  requirement: "States an answer",
};

function sample(): ReturnType<typeof initialTaskState> {
  return initialTaskState({
    id: "task-1",
    input: "What is X?",
    target: { text: "" },
    metadata: { criteria: [CRITERION] },
  });
}

async function runSolver(
  dir: string,
  config: DracoPanelConfig
): Promise<{
  calls: string[];
  state: ReturnType<typeof initialTaskState>;
}> {
  const calls: string[] = [];
  const responsesLayer = fixtureResponsesLayer(
    (m) => calls.push(m),
    config.analysisModels
  );
  const artifactLayer = makeFsArtifactStoreLayer(dir);
  const infraLayer = mergeAll(responsesLayer, artifactLayer);
  const solverLayer = effect(Solver)(
    gen(function* () {
      const responses = yield* Responses;
      const artifactStore = yield* ArtifactStoreService;
      return Solver.of(dracoSolver(responses, artifactStore, { config }));
    })
  );
  const state = await runPromise(
    gen(function* () {
      const solver = yield* Solver;
      return yield* solver(sample());
    }).pipe(
      provide(
        mergeAll(
          solverLayer.pipe(layerProvide(infraLayer)),
          noopProgressLayer,
          noopCheckpointLayer
        )
      )
    )
  );
  return { calls, state };
}

function productionFusionConfig(
  overrides: Partial<DracoPanelConfig> = {}
): DracoPanelConfig {
  return {
    name: "prod-test",
    description: "",
    type: "fusion",
    synthesisModel: "synth/s",
    analysisModels: ["panel/a", "panel/b"],
    searchEngine: "exa",
    blockedDomains: [],
    judgeModel: "judge/j1",
    judgeRuns: 1,
    judgeReasoningEffort: "low",
    criterionConcurrency: 10,
    timeout: 1800,
    concurrency: 2,
    ...overrides,
  };
}
describe("dracoSolver production-fusion cache reuse", () => {
  it("first run bills one generation call + one judge call", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-prod-"));
    try {
      const { calls } = await runSolver(dir, productionFusionConfig());
      const genCalls = calls.filter((m) => !m.startsWith("judge/"));
      const judgeCalls = calls.filter((m) => m.startsWith("judge/"));
      expect(genCalls).toHaveLength(1);
      expect(judgeCalls).toHaveLength(1);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("an unchanged config re-runs nothing (full cache hit)", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-prod-"));
    try {
      await runSolver(dir, productionFusionConfig());
      const { calls } = await runSolver(dir, productionFusionConfig());
      expect(calls).toHaveLength(0);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("changing analysisModels invalidates generation + judge (full config hash)", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-prod-"));
    try {
      await runSolver(
        dir,
        productionFusionConfig({ analysisModels: ["panel/a", "panel/b"] })
      );
      const { calls } = await runSolver(
        dir,
        productionFusionConfig({ analysisModels: ["panel/a", "panel/c"] })
      );
      const genCalls = calls.filter((m) => !m.startsWith("judge/"));
      expect(genCalls).toHaveLength(1);
      expect(calls.filter((m) => m.startsWith("judge/"))).toHaveLength(1);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("re-judging with a different judge reuses the generation", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-prod-"));
    try {
      await runSolver(dir, productionFusionConfig({ judgeModel: "judge/j1" }));
      const { calls } = await runSolver(
        dir,
        productionFusionConfig({ judgeModel: "judge/j2" })
      );
      const genCalls = calls.filter((m) => !m.startsWith("judge/"));
      expect(genCalls).toHaveLength(0);
      expect(calls.filter((m) => m === "judge/j2")).toHaveLength(1);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
