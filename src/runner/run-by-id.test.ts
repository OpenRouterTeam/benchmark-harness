import { describe, expect, it } from "bun:test";

import { dieMessage, succeed } from "effect/Effect";
import {
  fail as layerFail,
  mergeAll,
  succeed as layerSucceed,
} from "effect/Layer";
import { fromIterable } from "effect/Stream";

import type { HostBenchmarkRunConfig } from "../benchmarks/benchmark-config";
import { mcqScorer } from "../benchmarks/scorers/mcq/scorer";
import type { Benchmark } from "../benchmarks/types";
import { MessageRole, ScoreValue } from "../harness/core";
import { Dataset } from "../harness/dataset";
import type { SampleOutcome } from "../harness/run";
import { Scorer } from "../harness/scorer";
import { generate, Solver } from "../harness/solver";
import { assertLeft, assertRight } from "../internal/testing";
import type {
  PartialOutcomesPayload,
  PartialOutcomeStoreService,
} from "../results/partial-outcome-store";
import { datasetSizeById, runBenchmarkById } from "./run-by-id";

const HOST_BENCHMARK: Benchmark<HostBenchmarkRunConfig> = {
  id: "host_benchmark",
  makeDatasetLayer: () =>
    layerSucceed(Dataset, {
      stream: () => fromIterable([]),
      size: succeed(7),
    }),
  makeLayer: () => layerFail(new Error("host benchmark used")),
  temperature: 0,
  defaultEpochs: 1,
};

const HOST_CONFIG = {
  benchmarkId: "host_benchmark",
  model: "host/model",
  options: { subset: "all" },
} as const;

describe("benchmark runner by id", () => {
  it("runs an injected host benchmark instead of the native registry", async () => {
    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: HOST_BENCHMARK,
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });

    assertLeft(result);
    expect(result.left).toContain("host benchmark used");
  });

  it("rejects a host config without an injected benchmark", async () => {
    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });

    assertLeft(result);
    expect(result.left).toContain("host benchmark is required");
  });

  it("rejects an injected benchmark for a native config", async () => {
    const result = await runBenchmarkById({
      benchmarkId: "search_hle",
      hostBenchmark: HOST_BENCHMARK,
      apiKey: "unused",
      benchmarkConfig: {
        benchmarkId: "search_hle",
        model: "host/model",
      },
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });

    assertLeft(result);
    expect(result.left).toContain("cannot be supplied for native benchmark");
  });

  it("resolves dataset size from an injected host benchmark", async () => {
    const result = await datasetSizeById(HOST_BENCHMARK.id, HOST_BENCHMARK);

    assertRight(result);
    expect(result.right).toBe(7);
  });

  it("rejects an injected benchmark whose id does not match", async () => {
    const mismatched = { ...HOST_BENCHMARK, id: "other_host_benchmark" };

    const runResult = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: mismatched,
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });
    const sizeResult = await datasetSizeById(HOST_BENCHMARK.id, mismatched);

    assertLeft(runResult);
    assertLeft(sizeResult);
    expect(runResult.left).toContain("Benchmark id mismatch");
    expect(sizeResult.left).toBe(runResult.left);
  });
});

const RUNNABLE_SAMPLES = [
  { id: "s-1", input: "Q1 target B", target: { text: "B" } },
  { id: "s-2", input: "Q2 target B", target: { text: "B" } },
] as const;

function makeRunnableHostBenchmark(
  solverCalls: string[]
): Benchmark<HostBenchmarkRunConfig> {
  const datasetLayer = layerSucceed(Dataset, {
    stream: () => fromIterable(RUNNABLE_SAMPLES),
    size: succeed(RUNNABLE_SAMPLES.length),
  });
  const solver = generate(
    {
      generate: (messages) => {
        const userMsg =
          messages.find((m) => m.role === MessageRole.User)?.content ?? "";
        solverCalls.push(userMsg);
        return succeed({
          completion: "Answer: B",
          message: { role: MessageRole.Assistant, content: "Answer: B" },
          generationTimeMs: 100,
        });
      },
    },
    { temperature: 0 }
  );
  return {
    ...HOST_BENCHMARK,
    makeLayer: () =>
      mergeAll(
        datasetLayer,
        layerSucceed(Solver, Solver.of(solver)),
        layerSucceed(Scorer, Scorer.of(mcqScorer))
      ),
  };
}

function priorOutcome(sampleId: string, epoch: number): SampleOutcome {
  return {
    sampleScore: {
      sampleId,
      epoch,
      score: {
        value: ScoreValue.Correct,
        answer: "B",
        explanation: "persisted",
      },
    },
  };
}

function makePartialStore(payload: PartialOutcomesPayload | null): {
  store: PartialOutcomeStoreService;
  removals: number[];
} {
  const removals: number[] = [];
  return {
    store: {
      read: () => Promise.resolve(payload),
      write: () => Promise.resolve(),
      remove: () => {
        removals.push(1);
        return Promise.resolve();
      },
    },
    removals,
  };
}

describe("runBenchmarkById partial outcome resume", () => {
  it("skips sample-epochs persisted under the same run scope", async () => {
    const solverCalls: string[] = [];
    const { store, removals } = makePartialStore({
      scope: { epochs: 1 },
      outcomes: [priorOutcome("s-1", 0)],
    });

    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: makeRunnableHostBenchmark(solverCalls),
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
      partialOutcomeStore: store,
    });

    assertRight(result);
    expect(solverCalls).toEqual(["Q2 target B"]);
    expect(result.right.result.sampleScores).toHaveLength(2);
    expect(removals).toHaveLength(1);
  });

  it("discards persisted outcomes from a mismatched run scope", async () => {
    const solverCalls: string[] = [];
    const { store } = makePartialStore({
      scope: { epochs: 2 },
      outcomes: [priorOutcome("s-1", 0)],
    });

    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: makeRunnableHostBenchmark(solverCalls),
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
      partialOutcomeStore: store,
    });

    assertRight(result);
    expect(solverCalls.toSorted()).toEqual(["Q1 target B", "Q2 target B"]);
    expect(result.right.result.sampleScores).toHaveLength(2);
  });

  it("starts fresh when the partial store read fails", async () => {
    const solverCalls: string[] = [];
    const store: PartialOutcomeStoreService = {
      read: () => Promise.reject(new Error("read failed")),
      write: () => Promise.resolve(),
      remove: () => Promise.resolve(),
    };

    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: makeRunnableHostBenchmark(solverCalls),
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
      partialOutcomeStore: store,
    });

    assertRight(result);
    expect(solverCalls).toHaveLength(2);
  });

  it("keeps the partial store when result persistence fails", async () => {
    const solverCalls: string[] = [];
    const { store, removals } = makePartialStore({
      scope: { epochs: 1 },
      outcomes: [priorOutcome("s-1", 0)],
    });

    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: makeRunnableHostBenchmark(solverCalls),
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
      partialOutcomeStore: store,
      resultStore: { write: () => dieMessage("results store down") },
    });

    assertRight(result);
    expect(result.right.resultsPath).toBeNull();
    expect(removals).toHaveLength(0);
  });

  it("removes the partial store after result persistence succeeds", async () => {
    const solverCalls: string[] = [];
    const { store, removals } = makePartialStore({
      scope: { epochs: 1 },
      outcomes: [priorOutcome("s-1", 0)],
    });

    const result = await runBenchmarkById({
      benchmarkId: HOST_BENCHMARK.id,
      hostBenchmark: makeRunnableHostBenchmark(solverCalls),
      apiKey: "unused",
      benchmarkConfig: HOST_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
      partialOutcomeStore: store,
      resultStore: { write: () => succeed("/tmp/results.parquet") },
    });

    assertRight(result);
    expect(result.right.resultsPath).toBe("/tmp/results.parquet");
    expect(removals).toHaveLength(1);
  });
});
