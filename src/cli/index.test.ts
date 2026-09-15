import { describe, expect, it } from "bun:test";

import { buildBenchmarkConfig, parseArgs } from ".";
describe("bench-harness CLI", () => {
  it("defaults --reasoning-effort to high", () => {
    expect(parseArgs([]).reasoningEffort).toBe("high");
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

  it("rejects non-numeric values for numeric flags", () => {
    expect(() => parseArgs(["--concurrency", "abc"])).toThrow(
      "--concurrency must be a valid number, got: abc"
    );
    expect(() => parseArgs(["--limit", "xyz"])).toThrow(
      "--limit must be a valid number, got: xyz"
    );
    expect(() => parseArgs(["--start", "foo"])).toThrow(
      "--start must be a valid number, got: foo"
    );
    expect(() => parseArgs(["--epochs", "bar"])).toThrow(
      "--epochs must be a valid number, got: bar"
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

  it("derives agent reasoning effort for ori lanes", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "xhigh",
    });
    expect(config).toMatchObject({
      reasoningEffort: "xhigh",
      agentReasoningEffort: "xhigh",
    });
  });

  it("preserves explicit ori agent reasoning effort", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: { agentReasoningEffort: "max" },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "low",
    });
    expect(config).toMatchObject({
      reasoningEffort: "low",
      agentReasoningEffort: "max",
    });
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
      reasoningEffort: args.reasoningEffort,
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
      reasoningEffort: "high",
    });
    expect(config).toMatchObject({
      benchmarkId: "terminal_bench",
      agent: "pi",
    });
  });

  it("defaults terminal_bench to the CLI reasoning effort", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "high",
    });
    expect(config).toMatchObject({
      benchmarkId: "terminal_bench",
      agentReasoningEffort: "high",
      oriChannel: "stable",
    });
  });

  it("treats an empty run identifier as unset rather than malformed", () => {
    const prev = process.env["BENCH_CHILD_WORKFLOW_ID"];
    process.env["BENCH_CHILD_WORKFLOW_ID"] = "";
    try {
      expect(() =>
        buildBenchmarkConfig({
          benchmarkId: "terminal_bench",
          model: "anthropic/claude-opus-5",
          panelConfig: undefined,
          artifactDir: undefined,
          endpointId: undefined,
          imageDetail: undefined,
          reasoningEffort: "high",
        })
      ).not.toThrow();
    } finally {
      if (prev === undefined) {
        delete process.env["BENCH_CHILD_WORKFLOW_ID"];
      } else {
        process.env["BENCH_CHILD_WORKFLOW_ID"] = prev;
      }
    }
  });

  it("rejects the removed legacy thinking field", () => {
    expect(() =>
      buildBenchmarkConfig({
        benchmarkId: "terminal_bench",
        model: "anthropic/claude-opus-5",
        panelConfig: { thinking: "high" },
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
        reasoningEffort: "high",
      })
    ).toThrow("Unknown terminal_bench solver-config option(s): thinking");
  });

  it("rejects a misspelled option instead of silently defaulting it", () => {
    expect(() =>
      buildBenchmarkConfig({
        benchmarkId: "terminal_bench",
        model: "anthropic/claude-opus-5",
        panelConfig: { agentReasoningEfort: "max" },
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
        reasoningEffort: "high",
      })
    ).toThrow("agentReasoningEfort");
  });

  it("still accepts every documented option and base inference override", () => {
    const config = buildBenchmarkConfig({
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      panelConfig: {
        agent: "claude",
        agentReasoningEffort: "xhigh",
        oriChannel: "alpha",
        systemPrompt: "terse",
        allowedTools: ["Bash"],
        isolateAgentConfig: true,
        taskSubset: ["fix-git"],
        maxTokens: 1000,
      },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "high",
    });
    expect(config).toMatchObject({
      agent: "claude",
      agentReasoningEffort: "xhigh",
      oriChannel: "alpha",
      maxTokens: 1000,
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
        reasoningEffort: "high",
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
      reasoningEffort: "high",
    });
    expect(config).toMatchObject({
      benchmarkId: "tau3_bench_banking",
      retrievalConfig: "bm25_grep",
    });
  });

  it("forwards solver-config options to gpqa_diamond, mmlu_pro, mmmu_pro_vision, and ifstruct", () => {
    for (const benchmarkId of [
      "gpqa_diamond",
      "mmlu_pro",
      "mmmu_pro_vision",
      "ifstruct",
    ] as const) {
      const config = buildBenchmarkConfig({
        benchmarkId,
        model: "openai/gpt-5",
        panelConfig: {
          providerOnly: ["together"],
          maxTokens: 2048,
          timeoutMs: 30000,
        },
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: benchmarkId === "mmmu_pro_vision" ? "high" : undefined,
        reasoningEffort: "low",
      });
      expect(config).toMatchObject({
        benchmarkId,
        providerOnly: ["together"],
        maxTokens: 2048,
        timeoutMs: 30000,
        reasoningEffort: "low",
        ...(benchmarkId === "mmmu_pro_vision" ? { imageDetail: "high" } : {}),
      });
    }
  });

  it("rejects unknown solver-config options for gpqa_diamond and mmlu_pro", () => {
    expect(() =>
      buildBenchmarkConfig({
        benchmarkId: "gpqa_diamond",
        model: "openai/gpt-5",
        panelConfig: { unknownOption: true },
        artifactDir: undefined,
        endpointId: undefined,
        imageDetail: undefined,
        reasoningEffort: "high",
      })
    ).toThrow("Unknown gpqa_diamond solver-config option(s): unknownOption");
  });
});
