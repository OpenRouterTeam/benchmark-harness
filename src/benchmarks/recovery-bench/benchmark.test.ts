import { describe, expect, it } from "bun:test";

import { flatMap, provide, runPromise, runPromiseExit } from "effect/Effect";
import { isFailure } from "effect/Exit";

import { Dataset } from "../../harness/dataset";
import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { BenchmarkRunConfigSchema } from "../benchmark-config";
import { RECOVERY_BENCH_BENCHMARK } from "./benchmark";

const datasetSize = Dataset.pipe(flatMap((d) => d.size));

describe("recovery-bench benchmark", () => {
  it("sizes the dataset from the configured subset instead of the defaults", async () => {
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "recovery_bench",
      model: "deepseek/deepseek-v4",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
      taskSubset: ["path-tracing-reverse"],
    });
    assertRight(config);
    const configured = RECOVERY_BENCH_BENCHMARK.makeDatasetLayerForConfig;
    expect(configured).toBeDefined();
    if (configured === undefined) {
      throw new Error("expected makeDatasetLayerForConfig");
    }
    const defaultSize = await runPromise(
      datasetSize.pipe(provide(RECOVERY_BENCH_BENCHMARK.makeDatasetLayer()))
    );
    const configuredSize = await runPromise(
      datasetSize.pipe(provide(configured(config.right)))
    );
    expect(defaultSize).toBe(56);
    expect(configuredSize).toBe(1);
  });

  it("fails the configured dataset layer for a mismatched benchmark config", async () => {
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "deepseek/deepseek-v4",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(config);
    const configured = RECOVERY_BENCH_BENCHMARK.makeDatasetLayerForConfig;
    if (configured === undefined) {
      throw new Error("expected makeDatasetLayerForConfig");
    }
    const exit = await runPromiseExit(
      datasetSize.pipe(provide(configured(config.right)))
    );
    expect(isFailure(exit)).toBe(true);
  });
});
