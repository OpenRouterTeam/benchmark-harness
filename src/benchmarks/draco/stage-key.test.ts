import { describe, expect, it } from "bun:test";

import type { DracoPanelConfig } from "./schemas";
import {
  generationConfigSha,
  judgeKey,
  productionFusionGenKey,
  promptSha,
  soloGenKey,
  stableStringify,
  toolSurfaceSha,
} from "./stage-key";

function makeConfig(
  overrides: Partial<DracoPanelConfig> = {}
): DracoPanelConfig {
  return {
    name: "test",
    description: "",
    type: "single",
    model: "openai/gpt-4o",
    fallbackModel: undefined,
    provider: undefined,
    synthesisModel: undefined,
    analysisModels: [],
    tools: undefined,
    searchEngine: "exa",
    blockedDomains: [],
    versionOverride: undefined,
    judgeModel: "google/gemini-3.1-pro-preview",
    judgeRuns: 1,
    judgeTemperature: undefined,
    judgeReasoningEffort: "low",
    criterionConcurrency: 10,
    timeout: 1800,
    concurrency: 5,
    ...overrides,
  };
}
describe("stableStringify", () => {
  it("produces identical output regardless of key order", () => {
    const a = { z: 1, a: 2, m: 3 };
    const b = { a: 2, m: 3, z: 1 };
    expect(stableStringify(a)).toBe(stableStringify(b));
  });
  it("handles nested objects with different key orders", () => {
    const a = { outer: { z: 1, a: 2 }, first: true };
    const b = { first: true, outer: { a: 2, z: 1 } };
    expect(stableStringify(a)).toBe(stableStringify(b));
  });
  it("handles arrays (order preserved)", () => {
    expect(stableStringify([1, 2, 3])).toBe("[1,2,3]");
    expect(stableStringify([1, 2, 3])).not.toBe(stableStringify([3, 2, 1]));
  });
  it("handles primitive values", () => {
    expect(stableStringify("hello")).toBe('"hello"');
    expect(stableStringify(42)).toBe("42");
    expect(stableStringify(null)).toBe("null");
    expect(stableStringify(true)).toBe("true");
  });
  it("produces valid JSON", () => {
    const obj = { b: [1, { d: "e", c: 3 }], a: true };
    const result = stableStringify(obj);
    expect(() => JSON.parse(result)).not.toThrow();
  });
});
describe("promptSha", () => {
  it("is deterministic", () => {
    expect(promptSha("test prompt")).toBe(promptSha("test prompt"));
  });
  it("differs for different prompts", () => {
    expect(promptSha("prompt A")).not.toBe(promptSha("prompt B"));
  });
  it("returns a 16-char hex string", () => {
    expect(promptSha("x")).toMatch(/^[0-9a-f]{16}$/);
  });
});
describe("toolSurfaceSha", () => {
  it("is deterministic", () => {
    const tools = [{ type: "web_search" }];
    expect(toolSurfaceSha(tools)).toBe(toolSurfaceSha(tools));
  });
  it("returns a 16-char hex string", () => {
    expect(toolSurfaceSha([])).toMatch(/^[0-9a-f]{16}$/);
  });
});
describe("generationConfigSha", () => {
  it("is deterministic and order-independent", () => {
    const config = makeConfig();
    expect(generationConfigSha(config)).toBe(generationConfigSha(config));
  });
  it("returns a 32-char hex string", () => {
    expect(generationConfigSha(makeConfig())).toMatch(/^[0-9a-f]{32}$/);
  });
  it("preserves the pre-removal production fusion cache hashes", () => {
    const config = makeConfig({
      name: "cache-compatibility-fixture",
      description: "Representative production fusion config",
      type: "fusion",
      model: "openai/gpt-4o",
      provider: { only: ["DeepInfra"], ignore: ["OpenAI"] },
      synthesisModel: "anthropic/claude-opus-4.1",
      analysisModels: ["openai/gpt-4o-mini", "google/gemini-2.5-flash"],
      blockedDomains: ["example.com"],
      versionOverride: "2026-07-25",
      judgeModel: "google/gemini-2.5-pro",
      judgeRuns: 2,
      judgeTemperature: 0.1,
      judgeReasoningEffort: "medium",
      criterionConcurrency: 4,
      concurrency: 3,
    });
    const prompt = "You are a rigorous research agent.";
    expect(generationConfigSha(config)).toBe(
      "cfb14b538b94fe63b007c2d6494a2a9b"
    );
    expect(
      productionFusionGenKey({ taskId: "cache-task-1", config, prompt }).key
    ).toBe("60248a9cbc4a3c63671d1bdbeb4caae7");
  });
});
describe("soloGenKey", () => {
  it("produces a generation stage key", () => {
    const key = soloGenKey({
      taskId: "task-1",
      config: makeConfig(),
      prompt: "solve this",
      tools: [],
    });
    expect(key.stage).toBe("generation");
    expect(key.key).toMatch(/^[0-9a-f]{32}$/);
  });
  it("differs when model changes", () => {
    const opts = {
      taskId: "task-1",
      prompt: "solve this",
      tools: [],
    };
    const a = soloGenKey({
      ...opts,
      config: makeConfig({ model: "openai/gpt-4o" }),
    });
    const b = soloGenKey({
      ...opts,
      config: makeConfig({ model: "anthropic/claude-sonnet-4" }),
    });
    expect(a.key).not.toBe(b.key);
  });
});
describe("productionFusionGenKey", () => {
  it("produces a generation stage key", () => {
    const key = productionFusionGenKey({
      taskId: "task-1",
      config: makeConfig({ name: "fusion-exp", type: "fusion" }),
      prompt: "solve this",
    });
    expect(key.stage).toBe("generation");
    expect(key.key).toMatch(/^[0-9a-f]{32}$/);
  });
});
describe("judgeKey", () => {
  const baseJudgeOpts: {
    generationKey: string;
    judgeModel: string;
    criterionId: string;
    runNum: number;
    judgePrompt: string;
    judgeTemperature: number | undefined;
    judgeReasoningEffort: string | undefined;
    versionOverride: string | undefined;
  } = {
    generationKey: "gen-key",
    judgeModel: "openai/gpt-4o",
    criterionId: "crit-1",
    runNum: 1,
    judgePrompt: "judge this",
    judgeTemperature: undefined,
    judgeReasoningEffort: undefined,
    versionOverride: undefined,
  };
  it("produces a judge stage key", () => {
    const key = judgeKey(baseJudgeOpts);
    expect(key.stage).toBe("judge");
    expect(key.key).toMatch(/^[0-9a-f]{32}$/);
  });
  it("differs when run number changes", () => {
    const a = judgeKey({ ...baseJudgeOpts, runNum: 1 });
    const b = judgeKey({ ...baseJudgeOpts, runNum: 2 });
    expect(a.key).not.toBe(b.key);
  });
  it("differs when judgeTemperature changes", () => {
    const a = judgeKey({ ...baseJudgeOpts, judgeTemperature: 0.2 });
    const b = judgeKey({ ...baseJudgeOpts, judgeTemperature: 0 });
    expect(a.key).not.toBe(b.key);
  });
  it("differs when judgeReasoningEffort changes", () => {
    const a = judgeKey({ ...baseJudgeOpts, judgeReasoningEffort: "low" });
    const b = judgeKey({ ...baseJudgeOpts, judgeReasoningEffort: "high" });
    expect(a.key).not.toBe(b.key);
  });
  it("treats undefined temperature the same as omitted", () => {
    const a = judgeKey({ ...baseJudgeOpts, judgeTemperature: undefined });
    const b = judgeKey(baseJudgeOpts);
    expect(a.key).toBe(b.key);
  });
});
