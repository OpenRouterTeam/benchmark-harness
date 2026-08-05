import { describe, expect, it } from "bun:test";

import { FetchHttpClient } from "@effect/platform";
import { runPromise, scoped } from "effect/Effect";
import { build, provide } from "effect/Layer";

import { assertLeft, assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { Tau3BenchBankingConfigSchema } from "../benchmark-config";
import { getBenchmarkMeta } from "../benchmark-meta";
import { getBenchmark } from "../registry";
import {
  TAU3_BENCH_BANKING_BENCHMARK,
  TAU3_BENCH_BANKING_ID,
} from "./benchmark";
describe("tau3_bench_banking registration", () => {
  it("resolves from the registry", () => {
    const benchmark = getBenchmark("tau3_bench_banking");
    expect(benchmark).toBe(TAU3_BENCH_BANKING_BENCHMARK);
    expect(TAU3_BENCH_BANKING_ID).toBe("tau3_bench_banking");
    expect(TAU3_BENCH_BANKING_BENCHMARK.userModel).toBe("openai/gpt-5.4-mini");
  });
  it("has meta with defaultEpochs 5", () => {
    const meta = getBenchmarkMeta("tau3_bench_banking");
    expect(meta?.defaultEpochs).toBe(5);
  });
  it("resolves AA-parity banking defaults", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
    });
    assertRight(result);
    expect(result.right.userModel).toBe("openai/gpt-5.4-mini");
    expect(result.right.retrievalConfig).toBe("bm25_grep");
    expect(result.right.userReasoningEffort).toBe("medium");
  });
  it("treats an empty user model as unset", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      userModel: "",
    });
    assertRight(result);
    expect(result.right.userModel).toBe("openai/gpt-5.4-mini");
  });
  it("treats a whitespace-only user model as unset", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      userModel: "  \t",
    });
    assertRight(result);
    expect(result.right.userModel).toBe("openai/gpt-5.4-mini");
  });
  it("allows overriding the user simulator reasoning effort", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      userReasoningEffort: "high",
    });
    assertRight(result);
    expect(result.right.userReasoningEffort).toBe("high");
  });
  it("parses a valid config", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      userModel: "google/gemini-2.5-flash",
      retrievalConfig: "required_docs",
    });
    assertRight(result);
    expect(result.right.model).toBe("openai/gpt-4o-mini");
    expect(result.right.retrievalConfig).toBe("required_docs");
  });
  it("parses the bm25_grep retrieval config", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      retrievalConfig: "bm25_grep",
    });
    assertRight(result);
    expect(result.right.retrievalConfig).toBe("bm25_grep");
  });
  it("rejects a mismatched benchmarkId", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau_bench_verified_airline",
      model: "openai/gpt-4o-mini",
    });
    assertLeft(result);
  });
  it("rejects an unknown retrievalConfig", () => {
    const result = parseSchema(Tau3BenchBankingConfigSchema, {
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      retrievalConfig: "bm25",
    });
    assertLeft(result);
  });
  it("makeLayer rejects a mismatched config", async () => {
    const layer = TAU3_BENCH_BANKING_BENCHMARK.makeLayer({
      apiKey: "test",
      benchmarkConfig: {
        benchmarkId: "gpqa_diamond",
        model: "openai/gpt-4o-mini",
      },
      sessionId: "test-session",
    });
    const buildLayer = build(layer.pipe(provide(FetchHttpClient.layer))).pipe(
      scoped
    );
    await expect(runPromise(buildLayer)).rejects.toThrow(
      "invalid benchmarkConfig"
    );
  });
});
