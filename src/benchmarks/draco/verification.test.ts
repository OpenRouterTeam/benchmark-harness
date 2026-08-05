import { describe, expect, it } from "bun:test";

import type { DracoPanelConfig } from "./schemas";
import {
  isContentRefusal,
  isFusionPanelRefusal,
  summarizeToolInvocations,
  verifyFusion,
} from "./verification";
describe("isContentRefusal", () => {
  it("returns true for completed empty response with no tool invocations", () => {
    expect(isContentRefusal("", "completed", [])).toBe(true);
  });
  it("returns true for whitespace-only completed response with no tool invocations", () => {
    expect(isContentRefusal("   ", "completed", [])).toBe(true);
  });
  it("returns false when status is not completed", () => {
    expect(isContentRefusal("", "incomplete", [])).toBe(false);
  });
  it("returns false when content is present", () => {
    expect(isContentRefusal("Some answer", "completed", [])).toBe(false);
  });
  it("returns false when tool invocations are present (truncated web_fetch)", () => {
    expect(isContentRefusal("", "completed", [{ type: "web_fetch" }])).toBe(
      false
    );
  });
});
describe("isFusionPanelRefusal", () => {
  it("returns false for null fusion item", () => {
    expect(isFusionPanelRefusal(null)).toBe(false);
  });
  it("returns false when failed_models is empty", () => {
    expect(isFusionPanelRefusal({ failed_models: [] })).toBe(false);
  });
  it("returns false when failed_models is not an array", () => {
    expect(isFusionPanelRefusal({ failed_models: "not-array" })).toBe(false);
  });
  it("returns true when all failed models are Fable with empty-text marker", () => {
    expect(
      isFusionPanelRefusal({
        failed_models: [
          {
            model: "anthropic/claude-fable",
            error: "Model without producing any text",
          },
        ],
      })
    ).toBe(true);
  });
  it("returns false when a non-Fable model failed", () => {
    expect(
      isFusionPanelRefusal({
        failed_models: [
          { model: "openai/gpt-4o", error: "Model without producing any text" },
        ],
      })
    ).toBe(false);
  });
  it("returns false when Fable failed with a different error", () => {
    expect(
      isFusionPanelRefusal({
        failed_models: [{ model: "anthropic/claude-fable", error: "Timeout" }],
      })
    ).toBe(false);
  });
});
describe("verifyFusion", () => {
  const baseConfig: DracoPanelConfig = {
    name: "test",
    description: "",
    type: "fusion",
    model: undefined,
    fallbackModel: undefined,
    provider: undefined,
    synthesisModel: undefined,
    analysisModels: ["openai/gpt-4o", "anthropic/claude-sonnet-4"],
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
  it("returns error when no fusion output items exist", () => {
    const [, err] = verifyFusion([], baseConfig);
    expect(err).toContain("No openrouter:fusion output item");
  });
  it("returns error when no fusion item completed", () => {
    const output = [{ type: "openrouter:fusion", status: "failed" }];
    const [item, err] = verifyFusion(output, baseConfig);
    expect(err).toContain("No openrouter:fusion item completed");
    expect(item).not.toBeNull();
  });
  it("returns null error when analysis_models is empty (any panel accepted)", () => {
    const output = [
      { type: "openrouter:fusion", status: "completed", responses: [] },
    ];
    const [item, err] = verifyFusion(output, {
      ...baseConfig,
      analysisModels: [],
    });
    expect(err).toBeNull();
    expect(item).not.toBeNull();
  });
  it("returns null error when panel matches configured analysis_models", () => {
    const output = [
      {
        type: "openrouter:fusion",
        status: "completed",
        responses: [
          { model: "openai/gpt-4o" },
          { model: "anthropic/claude-sonnet-4" },
        ],
      },
    ];
    const [item, err] = verifyFusion(output, baseConfig);
    expect(err).toBeNull();
    expect(item).not.toBeNull();
  });
  it("returns panel mismatch error when models differ", () => {
    const output = [
      {
        type: "openrouter:fusion",
        status: "completed",
        responses: [
          { model: "openai/gpt-4o" },
          { model: "google/gemini-2.5-flash" },
        ],
      },
    ];
    const [, err] = verifyFusion(output, baseConfig);
    expect(err).toContain("Panel mismatch");
  });
});
describe("summarizeToolInvocations", () => {
  it("filters out message, reasoning, and fusion items", () => {
    const output = [
      { type: "message", status: "completed" },
      { type: "reasoning", status: "completed" },
      { type: "openrouter:fusion", status: "completed" },
      { type: "web_search", status: "completed", action: "search" },
    ];
    const result = summarizeToolInvocations(output);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({
      type: "web_search",
      status: "completed",
      action: "search",
    });
  });
  it("returns empty array when only message/reasoning/fusion items", () => {
    const output = [
      { type: "message", status: "completed" },
      { type: "reasoning", status: "completed" },
    ];
    expect(summarizeToolInvocations(output)).toEqual([]);
  });
  it("defaults status and action to null when missing", () => {
    const result = summarizeToolInvocations([{ type: "web_fetch" }]);
    expect(result[0]).toEqual({
      type: "web_fetch",
      status: null,
      action: null,
    });
  });
});
