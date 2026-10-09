import { describe, expect, it } from "bun:test";

import { assertLeft, assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import {
  BENCHMARK_OPTIONS_SCHEMAS,
  KeplerBenchmarkRunConfigSchema,
} from "./benchmark-config";
import { getBenchmarkMeta } from "./benchmark-meta";
import { getBenchmark } from "./registry";

describe("native eval migration", () => {
  it("does not expose Tau3 Banking through Kepler", () => {
    const benchmarkId = "tau3_bench_banking";
    expect(getBenchmark(benchmarkId)).toBeUndefined();
    expect(getBenchmarkMeta(benchmarkId)).toBeUndefined();
    expect(Object.keys(BENCHMARK_OPTIONS_SCHEMAS)).not.toContain(benchmarkId);
    assertLeft(
      parseSchema(KeplerBenchmarkRunConfigSchema, {
        benchmarkId,
        model: "openai/gpt-5",
        reasoningEffort: "high",
      })
    );
  });

  it.each([
    "gpqa_diamond",
    "mmlu_pro",
    "mmmu_pro_vision",
    "tau_bench_verified_airline",
    "vgi_bench",
  ])("retains %s", (benchmarkId) => {
    expect(getBenchmark(benchmarkId)).toBeDefined();
    expect(getBenchmarkMeta(benchmarkId)).toBeDefined();
    assertRight(
      parseSchema(KeplerBenchmarkRunConfigSchema, {
        benchmarkId,
        model: "openai/gpt-5",
        reasoningEffort: "high",
      })
    );
  });
});
