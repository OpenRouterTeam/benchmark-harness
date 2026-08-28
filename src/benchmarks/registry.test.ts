import { afterEach, describe, expect, it } from "bun:test";

import { blockNetwork } from "../../test/helpers/block-network";
import { NOOP_PROGRESS_REPORTER } from "../harness/progress";
import { assertRight, assertLeft } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import { runBenchmarkById } from "../runner/run-by-id";
import {
  BenchmarkRunConfigSchema,
  isSearchBenchmarkConfig,
} from "./benchmark-config";
import { getBenchmarkMeta } from "./benchmark-meta";
import { benchmarkIds, getBenchmark } from "./registry";
describe("benchmark registry", () => {
  let restoreNetwork: (() => void) | undefined;
  afterEach(() => {
    restoreNetwork?.();
    restoreNetwork = undefined;
  });

  it("resolves gpqa_diamond with a complete definition", () => {
    const b = getBenchmark("gpqa_diamond");
    expect(b).toBeDefined();
    expect(b?.id).toBe("gpqa_diamond");
    expect(b?.temperature).toBe(0.5);
    expect(b?.defaultEpochs).toBe(10);
    expect(typeof b?.makeLayer).toBe("function");
    expect(typeof b?.makeDatasetLayer).toBe("function");
  });

  it("dispatches runBenchmarkById through the registry entry", async () => {
    restoreNetwork = blockNetwork();
    const result = await runBenchmarkById({
      benchmarkId: "gpqa_diamond",
      apiKey: "unused",
      benchmarkConfig: {
        benchmarkId: "gpqa_diamond",
        model: "test/model",
        reasoningEffort: "high",
      },
      epochs: 1,
      maxConcurrency: 1,
      range: { start: 0, end: 1 },
      datasetRetry: { baseDelayMs: 0 },
      sessionId: "test",
      progressReporter: NOOP_PROGRESS_REPORTER,
    });
    assertLeft(result);
  });

  it("resolves all swe-atlas tracks as distinct benchmarks", () => {
    for (const id of [
      "swe_atlas_qa",
      "swe_atlas_tw",
      "swe_atlas_rf",
    ] as const) {
      const b = getBenchmark(id);
      expect(b?.id).toBe(id);
      expect(typeof b?.makeLayer).toBe("function");
      expect(typeof b?.makeDatasetLayer).toBe("function");
    }
  });

  it("registers search_hle with the default search lane", () => {
    const benchmark = getBenchmark("search_hle");
    expect(benchmark?.id).toBe("search_hle");
    expect(benchmark?.defaultEpochs).toBe(1);
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
    });
    assertRight(config);
    expect(config.right).toEqual({
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
      lane: { webSearch: "server-tool", engine: "auto" },
    });
  });

  it("registers search_dsqa with workflow metadata and the default search lane", () => {
    const benchmark = getBenchmark("search_dsqa");
    expect(benchmark?.id).toBe("search_dsqa");
    expect(benchmark?.defaultEpochs).toBe(1);
    expect(typeof benchmark?.runLevelScores).toBe("function");
    expect(typeof benchmark?.primaryScore).toBe("function");
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "search_dsqa",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
    });
    assertRight(config);
    expect(config.right).toEqual({
      benchmarkId: "search_dsqa",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
      lane: { webSearch: "server-tool", engine: "auto" },
    });
  });

  it("narrows parsed benchmark configs to the search family", () => {
    expect(
      isSearchBenchmarkConfig({
        benchmarkId: "search_dsqa",
        model: "model",
        reasoningEffort: "high",
        lane: { webSearch: "server-tool", engine: "auto" },
      })
    ).toBe(true);
    expect(
      isSearchBenchmarkConfig({ benchmarkId: "gpqa_diamond", model: "model" })
    ).toBe(false);
  });

  it("registers WideSearch with the default search lane", () => {
    const benchmark = getBenchmark("search_widesearch");
    expect(benchmark?.id).toBe("search_widesearch");
    expect(benchmark?.defaultEpochs).toBe(1);
    expect(typeof benchmark?.runLevelScores).toBe("function");
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "search_widesearch",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
    });
    assertRight(result);
    expect(result.right).toEqual({
      benchmarkId: "search_widesearch",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
      lane: { webSearch: "server-tool", engine: "auto" },
    });
  });

  it("registers WANDR with research-tool defaults and fractional scoring", () => {
    const benchmark = getBenchmark("wandr");
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "wandr",
      model: "openai/gpt-5.5",
      reasoningEffort: "high",
    });
    assertRight(config);
    expect(benchmark?.defaultEpochs).toBe(1);
    expect(typeof benchmark?.primaryScore).toBe("function");
    expect(config.right).toEqual({
      benchmarkId: "wandr",
      model: "openai/gpt-5.5",
      reasoningEffort: "high",
      modalEnv: "main",
      stepLimit: 64,
      serverTools: [
        { type: "openrouter:web_search" },
        { type: "openrouter:web_fetch" },
      ],
    });
  });

  it("parses a swe-atlas config and fills judge/step/modal defaults", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "swe_atlas_qa",
      model: "anthropic/claude-opus-4.5",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    if (result.right.benchmarkId !== "swe_atlas_qa") {
      throw new Error("expected swe_atlas_qa config");
    }
    expect(result.right.judgeModel).toBe("anthropic/claude-opus-4.5");
    expect(result.right.stepLimit).toBe(250);
    expect(result.right.modalEnv).toBe("main");
  });

  it("parses terminal-bench with the pi agent and the ori install url", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    if (result.right.benchmarkId !== "terminal_bench") {
      throw new Error("expected terminal_bench config");
    }
    expect(result.right.agent).toBe("pi");
    expect(result.right.oriInstallUrl).toBe(
      "https://openrouter.ai/labs/ori/install.sh"
    );
    expect(result.right.oriChannel).toBe("stable");
  });

  it("parses an ori agent selection for terminal-bench", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      agent: "claude",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    if (result.right.benchmarkId !== "terminal_bench") {
      throw new Error("expected terminal_bench config");
    }
    expect(result.right.agent).toBe("claude");
  });

  it("accepts Prime Agent for terminal-bench and harbor benchmarks", () => {
    for (const benchmarkId of ["terminal_bench", "deep_swe"] as const) {
      const result = parseSchema(BenchmarkRunConfigSchema, {
        benchmarkId,
        model: "openai/gpt-5.4",
        agent: "prime-agent",
        reasoningEffort: "high",
        agentReasoningEffort: "high",
      });
      assertRight(result);
      expect(result.right.agent).toBe("prime-agent");
    }
  });

  it("exposes the full terminal-bench agent control surface with defaults", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    if (result.right.benchmarkId !== "terminal_bench") {
      throw new Error("expected terminal_bench config");
    }
    expect(result.right.agentReasoningEffort).toBe("high");
    expect(result.right.oriChannel).toBe("stable");
    expect(result.right.isolateAgentConfig).toBe(false);
    expect(result.right.systemPrompt).toBeUndefined();
    expect(result.right.allowedTools).toBeUndefined();
    expect(result.right.disallowedTools).toBeUndefined();
  });

  it("accepts the max reasoning level", () => {
    const parsed = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      agentReasoningEffort: "max",
      reasoningEffort: "high",
    });
    assertRight(parsed);
    if (parsed.right.benchmarkId !== "terminal_bench") {
      throw new Error("expected terminal_bench config");
    }
    expect(parsed.right.agentReasoningEffort).toBe("max");
  });

  it("accepts none as an agent reasoning effort, which ori maps per harness", () => {
    const parsed = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      agentReasoningEffort: "none",
      reasoningEffort: "high",
    });
    assertRight(parsed);
    if (parsed.right.benchmarkId !== "terminal_bench") {
      throw new Error("expected terminal_bench config");
    }
    expect(parsed.right.agentReasoningEffort).toBe("none");
  });

  it("rejects an unknown agent reasoning effort", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      agentReasoningEffort: "off",
      reasoningEffort: "high",
    });
    assertLeft(result);
  });

  it("parses tool allow and deny lists", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "anthropic/claude-opus-5",
      allowedTools: ["Bash", "Edit"],
      reasoningEffort: "high",
      agentReasoningEffort: "high",
      disallowedTools: ["WebSearch"],
      isolateAgentConfig: true,
      systemPrompt: "terse",
    });
    assertRight(result);
    if (result.right.benchmarkId !== "terminal_bench") {
      throw new Error("expected terminal_bench config");
    }
    expect(result.right.allowedTools).toEqual(["Bash", "Edit"]);
    expect(result.right.disallowedTools).toEqual(["WebSearch"]);
    expect(result.right.isolateAgentConfig).toBe(true);
    expect(result.right.systemPrompt).toBe("terse");
  });

  it("rejects an agent that is not wired up", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench",
      model: "openai/gpt-5.4",
      agent: "codex",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertLeft(result);
  });

  it("defaults harbor benchmarks to the mini-swe agent loop", () => {
    for (const benchmarkId of [
      "swe_atlas_qa",
      "swe_atlas_tw",
      "swe_atlas_rf",
      "deep_swe",
    ] as const) {
      const result = parseSchema(BenchmarkRunConfigSchema, {
        benchmarkId,
        model: "anthropic/claude-opus-4.5",
        reasoningEffort: "high",
        agentReasoningEffort: "high",
      });
      assertRight(result);
      if (!("agent" in result.right)) {
        throw new Error(`${benchmarkId} config is missing an agent field`);
      }
      expect(result.right.agent).toBe("mini_swe");
    }
  });

  it("accepts an ori agent for harbor benchmarks", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "deep_swe",
      model: "anthropic/claude-opus-5",
      agent: "claude",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
      isolateAgentConfig: true,
    });
    assertRight(result);
    if (result.right.benchmarkId !== "deep_swe") {
      throw new Error("expected deep_swe config");
    }
    expect(result.right.agent).toBe("claude");
    expect(result.right.agentReasoningEffort).toBe("high");
    expect(result.right.isolateAgentConfig).toBe(true);
  });

  it("accepts pi as a harbor agent now that ori launches it", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "deep_swe",
      model: "anthropic/claude-opus-5",
      agent: "pi",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    if (result.right.benchmarkId !== "deep_swe") {
      throw new Error("expected deep_swe config");
    }
    expect(result.right.agent).toBe("pi");
  });

  it("rejects an agent no harness implements", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "deep_swe",
      model: "anthropic/claude-opus-5",
      agent: "codex",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertLeft(result);
  });

  it("leaves wandr without an agent selector", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "wandr",
      model: "openai/gpt-5.5",
      reasoningEffort: "high",
    });
    assertRight(result);
    expect("agent" in result.right).toBe(false);
  });

  it("returns undefined for an unknown benchmark", () => {
    expect(getBenchmark("does_not_exist")).toBeUndefined();
  });

  it("lists registered benchmark ids", () => {
    expect(benchmarkIds()).toContain("gpqa_diamond");
    expect(benchmarkIds()).toContain("draco");
  });

  it("meta mirrors registry id + defaultEpochs for every benchmark", () => {
    for (const id of benchmarkIds()) {
      const b = getBenchmark(id);
      const meta = getBenchmarkMeta(id);
      expect(meta).toBeDefined();
      expect(meta?.id).toBe(b?.id);
      expect(meta?.defaultEpochs).toBe(b?.defaultEpochs);
    }
  });

  it("meta and registry agree on the registered id set", () => {
    const metaIds = benchmarkIds().filter(
      (id) => getBenchmarkMeta(id) !== undefined
    );
    expect(metaIds).toEqual([...benchmarkIds()]);
  });
});
