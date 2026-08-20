import { describe, expect, it } from "bun:test";

import { ProviderSort, WebSearchEngine } from "../../../internal/enums";
import { searchSolverOptionsFromConfig } from "./benchmark";
import { buildSearchRequestBody } from "./request";
describe("searchSolverOptionsFromConfig", () => {
  const baseConfig = {
    benchmarkId: "search_hle",
    model: "model",
    lane: { webSearch: "server-tool", engine: "auto" },
  } as const;

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
      providerOrder: ["openai", "azure"],
      providerOnly: ["openai", "azure"],
      providerIgnore: ["bedrock"],
      allowFallbacks: false,
      cloudflareVersion: "worker-version",
    } as const;
    const options = searchSolverOptionsFromConfig({
      config,
      instructions: "instructions",
      temperature: 0,
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
      providerOrder: ["openai", "azure"],
      providerOnly: ["openai", "azure"],
      providerIgnore: ["bedrock"],
      allowFallbacks: false,
      versionOverride: "worker-version",
      retry: { maxRetries: 2, baseDelayMs: 3 },
    });
    expect(
      buildSearchRequestBody({ ...options, problem: "Q?" }).temperature
    ).toBe(0.2);
  });
  it("uses the benchmark-declared temperature when the config omits an override", () => {
    const options = searchSolverOptionsFromConfig({
      config: baseConfig,
      instructions: "instructions",
      temperature: 0,
      maxOutputTokens: 999,
    });
    expect(options.maxOutputTokens).toBe(999);
    expect(options.temperature).toBe(0);
    expect(
      buildSearchRequestBody({ ...options, problem: "Q?" }).temperature
    ).toBe(0);
  });

  it("clamps the default output tokens to the supplied ceiling", () => {
    const options = searchSolverOptionsFromConfig({
      config: baseConfig,
      instructions: "instructions",
      temperature: 0,
      maxOutputTokensCeiling: 32768,
    });

    expect(options.maxOutputTokens).toBe(32768);
  });

  it("clamps configured output tokens to the supplied ceiling", () => {
    const options = searchSolverOptionsFromConfig({
      config: { ...baseConfig, maxTokens: 64000 },
      instructions: "instructions",
      temperature: 0,
      maxOutputTokensCeiling: 16000,
    });

    expect(options.maxOutputTokens).toBe(16000);
  });

  it("preserves configured output tokens below the supplied ceiling", () => {
    const options = searchSolverOptionsFromConfig({
      config: { ...baseConfig, maxTokens: 8000 },
      instructions: "instructions",
      temperature: 0,
      maxOutputTokensCeiling: 16000,
    });

    expect(options.maxOutputTokens).toBe(8000);
  });

  it("preserves output tokens when no ceiling is supplied", () => {
    const options = searchSolverOptionsFromConfig({
      config: baseConfig,
      instructions: "instructions",
      temperature: 0,
      maxOutputTokens: 999,
    });

    expect(options.maxOutputTokens).toBe(999);
  });
});
