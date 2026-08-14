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

  it("selects an ori agent harness for terminal_bench through the solver config", () => {
    const args = parseArgs([
      "--benchmark",
      "terminal_bench",
      "--model",
      "anthropic/claude-opus-5",
      "--solver-config",
      '{"agent":"claude"}',
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
      benchmarkId: "terminal_bench",
      agent: "claude",
    });
  });

  it("defaults terminal_bench to the pi agent", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
    });
    expect(config).toMatchObject({
      benchmarkId: "terminal_bench",
      agent: "pi",
    });
  });

  it("still accepts the legacy thinking field for terminal_bench", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: { thinking: "off", piPackage: "pi@9" },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
    });
    expect(config).toMatchObject({
      benchmarkId: "terminal_bench",
      thinking: "off",
      piPackage: "pi@9",
    });
  });

  it("defaults terminal_bench to the unified ori reasoning effort", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
    });
    expect(config).toMatchObject({
      benchmarkId: "terminal_bench",
      agentReasoningEffort: "medium",
      oriChannel: "stable",
    });
  });

  it("rejects an unknown terminal_bench agent", () => {
    expect(() =>
      buildBenchmarkConfig({
        benchmarkId: "terminal_bench",
        model: "anthropic/claude-opus-5",
        panelConfig: { agent: "not-an-agent" },
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
      })
    ).toThrow("Invalid terminal_bench config");
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
