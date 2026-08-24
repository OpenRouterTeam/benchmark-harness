import { describe, expect, it } from "bun:test";

import { succeed } from "effect/Effect";
import { fail as layerFail, succeed as layerSucceed } from "effect/Layer";
import { fromIterable } from "effect/Stream";

import type { HostBenchmarkRunConfig } from "../benchmarks/benchmark-config";
import type { Benchmark } from "../benchmarks/types";
import { Dataset } from "../harness/dataset";
import { assertLeft, assertRight } from "../internal/testing";
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
