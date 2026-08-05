import { describe, expect, it } from "bun:test";

import { ProviderSort, WebSearchEngine } from "../../../internal/enums";
import { searchSolverOptionsFromConfig } from "./benchmark";
import { buildSearchRequestBody } from "./request";
describe("searchSolverOptionsFromConfig", () => {
  it("projects every shared search inference option", () => {
    const config = {
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4-nano",
      endpointId: "endpoint",
      lane: {
        webSearch: "server-tool",
        engine: WebSearchEngine.Exa,
        maxAgentTurns: 3,
      },
      temperature: 0.2,
      maxTokens: 123,
      reasoningEffort: "high",
      costTier: "xhigh",
      costQualityTradeoff: 4,
      timeoutMs: 456,
      sort: ProviderSort.Latency,
      cloudflareVersion: "worker-version",
    } as const;
    const options = searchSolverOptionsFromConfig({
      config,
      instructions: "instructions",
      retry: {
        maxRetries: 2,
        baseDelayMs: 3,
      },
    });
    expect(options).toEqual({
      model: "openai/gpt-5.4-nano",
      instructions: "instructions",
      lane: { webSearch: "server-tool", engine: "exa", maxAgentTurns: 3 },
      maxOutputTokens: 123,
      temperature: 0.2,
      reasoningEffort: "high",
      costTier: "xhigh",
      costQualityTradeoff: 4,
      timeoutMs: 456,
      endpointId: "endpoint",
      sort: "latency",
      versionOverride: "worker-version",
      retry: { maxRetries: 2, baseDelayMs: 3 },
    });
    expect(
      buildSearchRequestBody({ ...options, problem: "Q?" }).temperature
    ).toBe(0.2);
  });
  it("omits temperature when the config omits an override", () => {
    const config = {
      benchmarkId: "search_hle",
      model: "model",
      lane: { webSearch: "server-tool", engine: "auto" },
    } as const;
    const options = searchSolverOptionsFromConfig({
      config,
      instructions: "instructions",
      maxOutputTokens: 999,
    });
    expect(options.maxOutputTokens).toBe(999);
    expect(options.temperature).toBeUndefined();
    expect(
      buildSearchRequestBody({ ...options, problem: "Q?" }).temperature
    ).toBeUndefined();
  });
});
