import { describe, expect, it } from "bun:test";

import { gen, provide, runPromiseExit } from "effect/Effect";
import { isFailure, isSuccess } from "effect/Exit";

import { Dataset } from "../../harness/dataset";
import type { BenchmarkRunConfig } from "../benchmark-config";
import { getBenchmarkMeta } from "../benchmark-meta";
import { getBenchmark } from "../registry";
import { TOOLCALL_SCHEMA_FUZZ_BENCHMARK } from "./benchmark";
import { TOOLCALL_SCHEMA_FUZZ_REGRESSIONS } from "./regressions";

function configFor(generator: string): BenchmarkRunConfig {
  return {
    benchmarkId: "toolcall_schema_fuzz",
    model: "test/model",
    reasoningEffort: "none",
    generator,
  } as BenchmarkRunConfig;
}

function datasetSize(generator: string) {
  const layer = TOOLCALL_SCHEMA_FUZZ_BENCHMARK.makeDatasetLayerForConfig?.(
    configFor(generator)
  );
  if (layer === undefined) {
    throw new Error("makeDatasetLayerForConfig is required");
  }
  return runPromiseExit(
    gen(function* () {
      const dataset = yield* Dataset;
      return yield* dataset.size;
    }).pipe(provide(layer))
  );
}

describe("TOOLCALL_SCHEMA_FUZZ_BENCHMARK", () => {
  it("is registered with its metadata", () => {
    const meta = getBenchmarkMeta("toolcall_schema_fuzz");
    expect(getBenchmark("toolcall_schema_fuzz")).toBe(
      TOOLCALL_SCHEMA_FUZZ_BENCHMARK
    );
    expect(TOOLCALL_SCHEMA_FUZZ_BENCHMARK.defaultEpochs).toBe(
      meta?.defaultEpochs
    );
    expect(TOOLCALL_SCHEMA_FUZZ_BENCHMARK.temperature).toBe(0);
  });

  it("builds the default realistic dataset", async () => {
    const exit = await datasetSize("realistic");
    expect(isSuccess(exit) ? exit.value : undefined).toBe(2668);
  });

  it("rejects a generator with no cases instead of running an empty benchmark", async () => {
    expect(TOOLCALL_SCHEMA_FUZZ_REGRESSIONS.entries).toHaveLength(0);
    const exit = await datasetSize("regressions");
    expect(isFailure(exit)).toBe(true);
  });
});
