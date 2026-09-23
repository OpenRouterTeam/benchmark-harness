import { describe, expect, it } from "bun:test";
import { PassThrough } from "node:stream";

import { promise, succeed } from "effect/Effect";
import { fail as layerFail, succeed as layerSucceed } from "effect/Layer";
import { fromEffect, fromIterable } from "effect/Stream";

import type { InjectedBenchmarkRunConfig } from "../benchmarks/benchmark-config";
import { defineSingleTurnBenchmark } from "../benchmarks/define-single-turn-benchmark";
import { gpqaScorer, gpqaSolver } from "../benchmarks/gpqa";
import type { Benchmark } from "../benchmarks/types";
import { makeGcsCacheStore } from "../datasets/cache-store";
import { Dataset } from "../harness/dataset";
import { assertLeft, assertRight } from "../internal/testing";
import {
  datasetSizeById,
  runBenchmarkById,
  warmDatasetById,
} from "./run-by-id";

const INJECTED_BENCHMARK: Benchmark<InjectedBenchmarkRunConfig> = {
  id: "injected_benchmark",
  makeDatasetLayer: () =>
    layerSucceed(Dataset, {
      stream: () => fromIterable([]),
      size: succeed(7),
    }),
  makeLayer: () => layerFail(new Error("injected benchmark used")),
  temperature: 0,
  defaultEpochs: 1,
};

const INJECTED_CONFIG = {
  benchmarkId: "injected_benchmark",
  model: "injected/model",
  options: { subset: "all" },
} as const;

describe("benchmark runner by id", () => {
  it("runs an injected benchmark instead of the native registry", async () => {
    const result = await runBenchmarkById({
      benchmarkId: INJECTED_BENCHMARK.id,
      injectedBenchmark: INJECTED_BENCHMARK,
      apiKey: "unused",
      benchmarkConfig: INJECTED_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });

    assertLeft(result);
    expect(result.left).toContain("injected benchmark used");
  });

  it("rejects an injected-id config without an injected benchmark", async () => {
    const result = await runBenchmarkById({
      benchmarkId: INJECTED_BENCHMARK.id,
      apiKey: "unused",
      benchmarkConfig: INJECTED_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });

    assertLeft(result);
    expect(result.left).toContain("injected benchmark is required");
  });

  it("rejects an injected benchmark for a native config", async () => {
    const result = await runBenchmarkById({
      benchmarkId: "search_hle",
      injectedBenchmark: INJECTED_BENCHMARK,
      apiKey: "unused",
      benchmarkConfig: {
        benchmarkId: "search_hle",
        model: "injected/model",
      },
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });

    assertLeft(result);
    expect(result.left).toContain("cannot be supplied for native benchmark");
  });

  it("resolves dataset size from an injected benchmark", async () => {
    const result = await datasetSizeById({
      benchmarkId: INJECTED_BENCHMARK.id,
      injectedBenchmark: INJECTED_BENCHMARK,
    });

    assertRight(result);
    expect(result.right).toBe(7);
  });

  it("warms an injected benchmark dataset range", async () => {
    const streamOptions: { start?: number; end?: number }[] = [];
    const samples = Array.from({ length: 4 }, (_, index) => ({
      id: `sample-${index}`,
      input: "unused",
      target: { text: "unused" },
    }));
    const benchmark: Benchmark<InjectedBenchmarkRunConfig> = {
      ...INJECTED_BENCHMARK,
      makeDatasetLayer: () =>
        layerSucceed(Dataset, {
          stream: (opts) => {
            streamOptions.push(opts ?? {});
            return fromIterable(samples);
          },
          size: succeed(samples.length),
        }),
    };

    const result = await warmDatasetById({
      benchmarkId: benchmark.id,
      benchmarkConfig: INJECTED_CONFIG,
      range: { start: 15, end: 19 },
      injectedBenchmark: benchmark,
    });

    assertRight(result);
    expect(result.right).toBe(4);
    expect(streamOptions).toEqual([{ start: 15, end: 19 }]);
  });

  it("fails when warming cannot write the dataset cache", async () => {
    const sample = {
      id: "sample-0",
      input: "unused",
      target: { text: "unused" },
    };
    const failingStore = makeGcsCacheStore({
      bucket: "b",
      client: {
        async downloadObject() {
          return undefined;
        },
        async uploadObject() {
          throw new Error("upload failed");
        },
        async openObjectReadStream() {
          return undefined;
        },
        openObjectWriteStream() {
          return new PassThrough();
        },
        async objectUpdatedMs() {
          return undefined;
        },
      },
    });
    const benchmark: Benchmark<InjectedBenchmarkRunConfig> = {
      ...INJECTED_BENCHMARK,
      makeDatasetLayer: () =>
        layerSucceed(Dataset, {
          stream: () =>
            fromEffect(
              promise(() =>
                failingStore
                  .writeJson("failed.json", { ok: false })
                  .then(() => sample)
              )
            ),
          size: succeed(1),
        }),
    };

    const result = await warmDatasetById({
      benchmarkId: benchmark.id,
      benchmarkConfig: INJECTED_CONFIG,
      range: { start: 0, end: 1 },
      injectedBenchmark: benchmark,
    });

    assertLeft(result);
    expect(result.left).toContain("cache write");
  });

  it("warms the config-selected dataset layer", async () => {
    const selectedConfigs: InjectedBenchmarkRunConfig[] = [];
    const samples = Array.from({ length: 2 }, (_, index) => ({
      id: `sample-${index}`,
      input: "unused",
      target: { text: "unused" },
    }));
    const benchmark = defineSingleTurnBenchmark({
      id: "warmable_injected_benchmark",
      temperature: 0,
      defaultEpochs: 1,
      isConfig: (config): config is InjectedBenchmarkRunConfig =>
        config.benchmarkId === "warmable_injected_benchmark",
      makeDatasetLayer: () =>
        layerSucceed(Dataset, {
          stream: () => fromIterable([]),
          size: succeed(0),
        }),
      makeDatasetLayerForConfig: (config) => {
        selectedConfigs.push(config);
        return layerSucceed(Dataset, {
          stream: () => fromIterable(samples),
          size: succeed(samples.length),
        });
      },
      scorer: gpqaScorer,
      makeSolver: (model) =>
        gpqaSolver(model, { inference: { reasoningEffort: "medium" } }),
    });
    const benchmarkConfig = {
      ...INJECTED_CONFIG,
      benchmarkId: benchmark.id,
    };

    const result = await warmDatasetById({
      benchmarkId: benchmark.id,
      benchmarkConfig,
      range: { start: 3, end: 5 },
      injectedBenchmark: benchmark,
    });

    assertRight(result);
    expect(result.right).toBe(2);
    expect(selectedConfigs).toEqual([benchmarkConfig]);
  });

  it("rejects a mismatched config in the config-selected dataset layer", async () => {
    const benchmark = defineSingleTurnBenchmark({
      id: "warmable_injected_benchmark",
      temperature: 0,
      defaultEpochs: 1,
      isConfig: (config): config is InjectedBenchmarkRunConfig =>
        config.benchmarkId === "warmable_injected_benchmark",
      makeDatasetLayer: () =>
        layerSucceed(Dataset, {
          stream: () => fromIterable([]),
          size: succeed(0),
        }),
      scorer: gpqaScorer,
      makeSolver: (model) =>
        gpqaSolver(model, { inference: { reasoningEffort: "medium" } }),
    });

    const result = await warmDatasetById({
      benchmarkId: benchmark.id,
      benchmarkConfig: INJECTED_CONFIG,
      range: { start: 0, end: 1 },
      injectedBenchmark: benchmark,
    });

    assertLeft(result);
    expect(result.left).toContain("received mismatched benchmarkConfig");
  });

  it("rejects an injected benchmark whose id does not match", async () => {
    const mismatched = {
      ...INJECTED_BENCHMARK,
      id: "other_injected_benchmark",
    };

    const runResult = await runBenchmarkById({
      benchmarkId: INJECTED_BENCHMARK.id,
      injectedBenchmark: mismatched,
      apiKey: "unused",
      benchmarkConfig: INJECTED_CONFIG,
      epochs: 1,
      maxConcurrency: 1,
      sessionId: "test",
    });
    const sizeResult = await datasetSizeById({
      benchmarkId: INJECTED_BENCHMARK.id,
      injectedBenchmark: mismatched,
    });

    assertLeft(runResult);
    assertLeft(sizeResult);
    expect(runResult.left).toContain("Benchmark id mismatch");
    expect(sizeResult.left).toBe(runResult.left);
  });

  it("resolves dataset size from the benchmark config", async () => {
    const benchmark: Benchmark<InjectedBenchmarkRunConfig> = {
      ...INJECTED_BENCHMARK,
      makeDatasetLayer: () =>
        layerSucceed(Dataset, {
          stream: () => fromIterable([]),
          size: succeed(100),
        }),
      makeDatasetLayerForConfig: () =>
        layerSucceed(Dataset, {
          stream: () => fromIterable([]),
          size: succeed(80),
        }),
    };

    const configured = await datasetSizeById({
      benchmarkId: benchmark.id,
      benchmarkConfig: INJECTED_CONFIG,
      injectedBenchmark: benchmark,
    });
    const defaulted = await datasetSizeById({
      benchmarkId: benchmark.id,
      injectedBenchmark: benchmark,
    });

    assertRight(configured);
    assertRight(defaulted);
    expect(configured.right).toBe(80);
    expect(defaulted.right).toBe(100);
  });
});
