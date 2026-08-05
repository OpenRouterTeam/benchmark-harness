import { describe, expect, it } from "bun:test";

import { buildBenchmarkConfig, parseArgs } from ".";
describe("bench-harness CLI", () => {
  it("parses and forwards --cost-tier", () => {
    const args = parseArgs([
      "--benchmark",
      "gpqa_diamond",
      "--model",
      "openrouter/auto",
      "--cost-tier",
      "xhigh",
    ]);
    expect(
      buildBenchmarkConfig({
        benchmarkId: args.benchmark,
        model: args.model,
        panelConfig: undefined,
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
        costTier: args.costTier,
      })
    ).toMatchObject({ costTier: "xhigh" });
  });
  it("rejects an invalid --cost-tier value", () => {
    expect(() => parseArgs(["--cost-tier", "invalid"])).toThrow(
      "--cost-tier must be one of"
    );
  });
  it("passes tau3 retrieval config through the generic solver config", () => {
    const args = parseArgs([
      "--benchmark",
      "tau3_bench_banking",
      "--model",
      "openai/gpt-4o-mini",
      "--solver-config",
      '{"retrievalConfig":"bm25_grep"}',
    ]);
    const panelConfig: unknown = JSON.parse(args.solverConfig ?? "");
    const config = buildBenchmarkConfig({
      benchmarkId: args.benchmark,
      model: args.model,
      panelConfig,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
    });
    expect(config).toMatchObject({
      benchmarkId: "tau3_bench_banking",
      retrievalConfig: "bm25_grep",
    });
  });
  it("materializes the bm25_grep default for tau3", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "tau3_bench_banking",
      model: "openai/gpt-4o-mini",
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
    });
    expect(config).toMatchObject({
      benchmarkId: "tau3_bench_banking",
      retrievalConfig: "bm25_grep",
    });
  });
});
