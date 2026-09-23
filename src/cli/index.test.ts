import { describe, expect, it } from "bun:test";

import { buildBenchmarkConfig, parseArgs, resolveSessionId } from ".";
describe("bench-harness CLI", () => {
  it("defaults --reasoning-effort to high", () => {
    expect(parseArgs([]).reasoningEffort).toBe("high");
    expect(parseArgs(["--model", "openai/gpt-5"]).reasoningEffort).toBe("high");
  });

  it("defaults --reasoning-effort to auto for openrouter/jev", () => {
    expect(parseArgs(["--model", "openrouter/jev"]).reasoningEffort).toBe(
      "auto"
    );
    expect(parseArgs(["--model", "openrouter/jev:nitro"]).reasoningEffort).toBe(
      "auto"
    );
    expect(
      parseArgs(["--model", "openrouter/jev", "--reasoning-effort", "high"])
        .reasoningEffort
    ).toBe("high");
    expect(
      parseArgs(["--model", "openrouter/jev", "--reasoning-effort", "auto"])
        .reasoningEffort
    ).toBe("auto");
  });

  it("rejects --reasoning-effort auto for models without adaptive effort", () => {
    expect(() =>
      parseArgs(["--model", "openai/gpt-5", "--reasoning-effort", "auto"])
    ).toThrow("--reasoning-effort auto is only supported for");
    expect(() => parseArgs(["--reasoning-effort", "auto"])).toThrow(
      "--reasoning-effort auto is only supported for"
    );
  });

  it("accepts an explicit --reasoning-effort", () => {
    expect(parseArgs(["--reasoning-effort", "low"]).reasoningEffort).toBe(
      "low"
    );
  });

  it("rejects an invalid --reasoning-effort value", () => {
    expect(() => parseArgs(["--reasoning-effort", "invalid"])).toThrow(
      "--reasoning-effort must be one of"
    );
  });

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
        reasoningEffort: args.reasoningEffort,
      })
    ).toMatchObject({ costTier: "xhigh" });
  });

  it("rejects an invalid --cost-tier value", () => {
    expect(() => parseArgs(["--cost-tier", "invalid"])).toThrow(
      "--cost-tier must be one of"
    );
  });

  it("passes reasoning effort to hand-built model benchmark configs", () => {
    for (const benchmarkId of [
      "gpqa_diamond",
      "mmlu_pro",
      "mmmu_pro_vision",
      "ifstruct",
    ] as const) {
      const config = buildBenchmarkConfig({
        benchmarkId,
        model: "openai/gpt-5",
        panelConfig: undefined,
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
        reasoningEffort: "low",
      });
      expect(config).toMatchObject({ reasoningEffort: "low" });
    }
  });

  it("forwards candidate models from --solver-config to single-turn benchmarks", () => {
    const args = parseArgs([
      "--benchmark",
      "gpqa_diamond",
      "--model",
      "openrouter/switchyard",
      "--solver-config",
      '{"models":["openai/gpt-4.1-nano","anthropic/claude-sonnet-4.5"]}',
    ]);
    const panelConfig: unknown = JSON.parse(args.solverConfig ?? "");
    const config = buildBenchmarkConfig({
      benchmarkId: args.benchmark,
      model: args.model,
      panelConfig,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: args.reasoningEffort,
    });
    expect(config).toEqual({
      benchmarkId: "gpqa_diamond",
      model: "openrouter/switchyard",
      models: ["openai/gpt-4.1-nano", "anthropic/claude-sonnet-4.5"],
      reasoningEffort: "high",
    });
  });

  it("keeps --image-detail when mmmu_pro_vision takes a solver config", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "mmmu_pro_vision",
      model: "openrouter/switchyard",
      panelConfig: { models: ["openai/gpt-4.1-nano", "openai/gpt-5"] },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: "high",
      reasoningEffort: "low",
    });
    expect(config).toMatchObject({
      benchmarkId: "mmmu_pro_vision",
      imageDetail: "high",
      models: ["openai/gpt-4.1-nano", "openai/gpt-5"],
    });
  });

  it("rejects unknown solver-config keys for single-turn benchmarks", () => {
    expect(() =>
      buildBenchmarkConfig({
        benchmarkId: "gpqa_diamond",
        model: "openai/gpt-5",
        panelConfig: { bogus: true },
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
        reasoningEffort: "low",
      })
    ).toThrow("Unknown gpqa_diamond solver-config option(s): bogus");
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
      reasoningEffort: args.reasoningEffort,
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
      reasoningEffort: "high",
    });
    expect(config).toMatchObject({
      benchmarkId: "tau3_bench_banking",
      retrievalConfig: "bm25_grep",
    });
  });
});

describe("resolveSessionId", () => {
  const withWorkflowId = <T>(value: string, run: () => T): T => {
    const prev = process.env["BENCH_CHILD_WORKFLOW_ID"];
    process.env["BENCH_CHILD_WORKFLOW_ID"] = value;
    try {
      return run();
    } finally {
      if (prev === undefined) {
        delete process.env["BENCH_CHILD_WORKFLOW_ID"];
      } else {
        process.env["BENCH_CHILD_WORKFLOW_ID"] = prev;
      }
    }
  };

  it("uses BENCH_CHILD_WORKFLOW_ID when set", () => {
    expect(withWorkflowId("trial-1", resolveSessionId)).toBe("trial-1");
  });

  it("treats an empty run identifier as unset rather than malformed", () => {
    expect(withWorkflowId("", resolveSessionId)).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("rejects control characters that would break the x-session-id header", () => {
    expect(() => withWorkflowId("trial-1\n", resolveSessionId)).toThrow(
      "control character"
    );
    expect(() => withWorkflowId("trial\u007F", resolveSessionId)).toThrow(
      "control character"
    );
  });
});
