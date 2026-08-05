import { describe, expect, it } from "bun:test";

import type { ResponsesResult } from "../../providers/responses-client";
import { buildGenerationResult, extractCost } from "./generation";
import type { DracoPanelConfig } from "./schemas";

const baseConfig: DracoPanelConfig = {
  name: "test-exp",
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
  judgeModel: "openai/gpt-4o",
  judgeRuns: 1,
  judgeTemperature: undefined,
  judgeReasoningEffort: "low",
  criterionConcurrency: 5,
  timeout: 1800,
  concurrency: 5,
};

function makeResult(overrides: Partial<ResponsesResult> = {}): ResponsesResult {
  return {
    id: "gen-123",
    status: "completed",
    text: "A detailed answer.",
    output: [{ type: "message", status: "completed" }],
    model: "openai/gpt-4o",
    usage: { cost: 0.01 },
    generationId: null,
    provider: null,
    generationTimeMs: 0,
    ...overrides,
  };
}
describe("extractCost", () => {
  it("returns null for null usage", () => {
    expect(extractCost(null)).toBeNull();
  });
  it("extracts numeric cost", () => {
    expect(extractCost({ cost: 0.05 })).toBe(0.05);
  });
  it("returns null when cost field is missing", () => {
    expect(extractCost({ tokens: 100 })).toBeNull();
  });
  it("returns null when cost is a string", () => {
    expect(extractCost({ cost: "0.05" })).toBeNull();
  });
  it("extracts zero cost", () => {
    expect(extractCost({ cost: 0 })).toBe(0);
  });
});
describe("buildGenerationResult", () => {
  it("returns ok status for a completed response with content", () => {
    const gen = buildGenerationResult({
      taskId: "task-1",
      config: baseConfig,
      result: makeResult(),
    });
    expect(gen.status).toBe("ok");
    expect(gen.content).toBe("A detailed answer.");
    expect(gen.model).toBe("openai/gpt-4o");
    expect(gen.cost).toBe(0.01);
  });
  it("returns failed status for non-completed response", () => {
    const gen = buildGenerationResult({
      taskId: "task-1",
      config: baseConfig,
      result: makeResult({ status: "incomplete" }),
    });
    expect(gen.status).toBe("failed");
    expect(gen.content).toBeNull();
    expect(gen.error).toContain("incomplete");
  });
  it("returns refused status for empty completed response with no tools", () => {
    const gen = buildGenerationResult({
      taskId: "task-1",
      config: baseConfig,
      result: makeResult({
        text: "",
        output: [{ type: "message", status: "completed" }],
      }),
    });
    expect(gen.status).toBe("refused");
    expect(gen.error).toContain("Content refusal");
  });
  it("returns failed status for empty response with tool invocations", () => {
    const gen = buildGenerationResult({
      taskId: "task-1",
      config: baseConfig,
      result: makeResult({
        text: "",
        output: [
          { type: "web_search", status: "completed", action: "search" },
          { type: "message", status: "completed" },
        ],
      }),
    });
    expect(gen.status).toBe("failed");
    expect(gen.error).toContain("Empty response content");
  });
  it("populates contaminationSignals when leak markers are present", () => {
    const gen = buildGenerationResult({
      taskId: "task-1",
      config: baseConfig,
      result: makeResult({ text: "see perplexity-ai/draco for details" }),
    });
    expect(gen.contaminationSignals.length).toBeGreaterThan(0);
    expect(gen.contaminationSignals).toContain(
      "leak-marker:perplexity-ai/draco"
    );
  });
  it("falls back to config model when result model is null", () => {
    const gen = buildGenerationResult({
      taskId: "task-1",
      config: baseConfig,
      result: makeResult({ model: null }),
    });
    expect(gen.model).toBe("openai/gpt-4o");
  });
});
