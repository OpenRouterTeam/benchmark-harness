import { describe, expect, it } from "bun:test";

import type { CliArgs, CliError } from ".";
import {
  buildBenchmarkConfig as buildBenchmarkConfigResult,
  formatCliError,
  parseArgs as parseArgsResult,
  resolveSessionId,
} from ".";
import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import { assertFailure, assertSuccess } from "../internal/testing";

function parseArgs(argv: readonly string[]): CliArgs {
  const parsed = parseArgsResult(argv);
  assertSuccess(parsed);
  return parsed.success;
}

function buildBenchmarkConfig(
  opts: Parameters<typeof buildBenchmarkConfigResult>[0]
): BenchmarkRunConfig {
  const built = buildBenchmarkConfigResult(opts);
  assertSuccess(built);
  return built.success;
}

function parseArgsFailure(argv: readonly string[]): CliError {
  const parsed = parseArgsResult(argv);
  assertFailure(parsed);
  return parsed.failure;
}

function buildBenchmarkConfigFailure(
  opts: Parameters<typeof buildBenchmarkConfigResult>[0]
): CliError {
  const built = buildBenchmarkConfigResult(opts);
  assertFailure(built);
  return built.failure;
}

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

  it("defaults --reasoning-effort to auto for typesafe/jev-router", () => {
    expect(parseArgs(["--model", "typesafe/jev-router"]).reasoningEffort).toBe(
      "auto"
    );
    expect(
      parseArgs([
        "--model",
        "typesafe/jev-router",
        "--reasoning-effort",
        "auto",
      ]).reasoningEffort
    ).toBe("auto");
  });

  it("rejects --reasoning-effort auto for models without adaptive effort", () => {
    const withModel = parseArgsFailure([
      "--model",
      "openai/gpt-5",
      "--reasoning-effort",
      "auto",
    ]);
    expect(withModel).toEqual({
      kind: "adaptive_reasoning_effort_unsupported",
      model: "openai/gpt-5",
    });
    expect(formatCliError(withModel)).toContain(
      "--reasoning-effort auto is only supported for"
    );
    const withoutModel = parseArgsFailure(["--reasoning-effort", "auto"]);
    expect(withoutModel).toEqual({
      kind: "adaptive_reasoning_effort_unsupported",
      model: undefined,
    });
    expect(formatCliError(withoutModel)).toContain(
      "--reasoning-effort auto is only supported for"
    );
  });

  it("accepts an explicit --reasoning-effort", () => {
    expect(parseArgs(["--reasoning-effort", "low"]).reasoningEffort).toBe(
      "low"
    );
  });

  it("rejects an invalid --reasoning-effort value", () => {
    const failure = parseArgsFailure(["--reasoning-effort", "invalid"]);
    expect(failure).toEqual({
      kind: "invalid_reasoning_effort",
      raw: "invalid",
    });
    expect(formatCliError(failure)).toContain(
      "--reasoning-effort must be one of"
    );
  });

  it("rejects an invalid --image-detail value", () => {
    const failure = parseArgsFailure(["--image-detail", "ultra"]);
    expect(failure).toEqual({ kind: "invalid_image_detail", raw: "ultra" });
    expect(formatCliError(failure)).toBe(
      '--image-detail must be one of: auto, low, high (got "ultra")'
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
    const failure = parseArgsFailure(["--cost-tier", "invalid"]);
    expect(failure).toEqual({ kind: "invalid_cost_tier", raw: "invalid" });
    expect(formatCliError(failure)).toContain("--cost-tier must be one of");
  });

  it("requires --model for model benchmarks", () => {
    const failure = buildBenchmarkConfigFailure({
      benchmarkId: "gpqa_diamond",
      model: undefined,
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "high",
    });
    expect(failure).toEqual({
      kind: "model_required",
      benchmarkId: "gpqa_diamond",
    });
    expect(formatCliError(failure)).toBe("gpqa_diamond requires --model");
  });

  it("rejects an unsupported benchmark id", () => {
    const failure = buildBenchmarkConfigFailure({
      benchmarkId: "not_a_benchmark",
      model: "openai/gpt-5",
      panelConfig: undefined,
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "high",
    });
    expect(failure).toEqual({
      kind: "unsupported_benchmark",
      benchmarkId: "not_a_benchmark",
    });
    expect(formatCliError(failure)).toBe(
      "Unsupported benchmark: not_a_benchmark"
    );
  });

  it("rejects an invalid DRACO panel config", () => {
    const failure = buildBenchmarkConfigFailure({
      benchmarkId: "draco",
      model: undefined,
      panelConfig: { panel: 42 },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "high",
    });
    expect(failure.kind).toBe("invalid_draco_panel_config");
    expect(formatCliError(failure)).toStartWith("Invalid DRACO panel config: ");
  });

  it("returns a schema validation failure for invalid benchmark options", () => {
    const failure = buildBenchmarkConfigFailure({
      benchmarkId: "gpqa_diamond",
      model: "openai/gpt-5",
      panelConfig: { maxTokens: "invalid" },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "high",
    });
    expect(failure.kind).toBe("invalid_benchmark_config");
    expect(formatCliError(failure)).toStartWith(
      "Invalid gpqa_diamond config: "
    );
  });

  it("passes reasoning effort to hand-built model benchmark configs", () => {
    for (const benchmarkId of [
      "gpqa_diamond",
      "mmlu_pro",
      "mmmu_pro_vision",
      "ifstruct",
      "toolcall_formats",
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
    const failure = buildBenchmarkConfigFailure({
      benchmarkId: "gpqa_diamond",
      model: "openai/gpt-5",
      panelConfig: { bogus: true },
      artifactDir: undefined,
      endpointId: undefined,
      imageDetail: undefined,
      reasoningEffort: "low",
    });
    expect(failure).toEqual({
      kind: "unknown_solver_config_options",
      benchmarkId: "gpqa_diamond",
      options: ["bogus"],
    });
    expect(formatCliError(failure)).toBe(
      "Unknown gpqa_diamond solver-config option(s): bogus"
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
    const result = withWorkflowId("trial-1", resolveSessionId);
    assertSuccess(result);
    expect(result.success).toBe("trial-1");
  });

  it("treats an empty run identifier as unset rather than malformed", () => {
    const result = withWorkflowId("", resolveSessionId);
    assertSuccess(result);
    expect(result.success).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("rejects control characters that would break the x-session-id header", () => {
    for (const sessionId of ["trial-1\n", "trial\u007F"]) {
      const result = withWorkflowId(sessionId, resolveSessionId);
      assertFailure(result);
      expect(result.failure).toEqual({ kind: "unsafe_session_id", sessionId });
      expect(formatCliError(result.failure)).toBe(
        `BENCH_CHILD_WORKFLOW_ID contains a control character, which is not a valid x-session-id header value (got ${JSON.stringify(sessionId)}).`
      );
    }
  });
});
