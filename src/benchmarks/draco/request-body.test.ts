import { describe, expect, it } from "bun:test";

import type {
  EasyInputMessage,
  FusionServerToolOpenRouter,
  InputsUnion,
  ResponsesRequest,
  ResponsesRequestToolUnion,
  ShellServerToolOpenRouter,
  WebFetchServerTool,
  WebSearchServerToolOpenRouter,
} from "@openrouter/sdk/models";

import { assertRight } from "../../internal/testing";
import { parseSchema, z } from "../../internal/zod";
import {
  buildFusionBody,
  buildSoloBody,
  experimentTools,
} from "./request-body";
import type { DracoPanelConfig } from "./schemas";

function config(overrides: Partial<DracoPanelConfig> = {}): DracoPanelConfig {
  return {
    name: "test",
    description: "",
    type: "single",
    model: "openai/gpt-4o-mini",
    fallbackModel: undefined,
    provider: undefined,
    synthesisModel: undefined,
    analysisModels: [],
    tools: undefined,
    searchEngine: "exa",
    blockedDomains: ["arxiv.org/abs/2602.11685"],
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

function isWebSearch(
  t: ResponsesRequestToolUnion
): t is WebSearchServerToolOpenRouter {
  return t.type === "openrouter:web_search";
}

function isWebFetch(t: ResponsesRequestToolUnion): t is WebFetchServerTool {
  return t.type === "openrouter:web_fetch";
}

function isShell(t: ResponsesRequestToolUnion): t is ShellServerToolOpenRouter {
  return t.type === "openrouter:shell";
}

function isFusion(
  t: ResponsesRequestToolUnion
): t is FusionServerToolOpenRouter {
  return t.type === "openrouter:fusion";
}

function isEasyInputMessage(item: unknown): item is EasyInputMessage {
  return item !== null && typeof item === "object" && "content" in item;
}

function firstInputContent(body: ResponsesRequest): string {
  const input: InputsUnion | undefined = body.input;
  if (typeof input === "string") {
    return input;
  }
  if (!Array.isArray(input)) {
    return "";
  }
  const first = input[0];
  if (isEasyInputMessage(first) && typeof first.content === "string") {
    return first.content;
  }
  return "";
}
describe("applyBlocklist", () => {
  it("injects engine + domain filter into web_search (excludedDomains) and web_fetch (blockedDomains)", () => {
    const cfg = config();
    const tools = experimentTools(cfg);
    const ws = tools.find(isWebSearch)!;
    const wf = tools.find(isWebFetch)!;
    expect(ws.parameters).toMatchObject({
      engine: "exa",
      excludedDomains: ["arxiv.org/abs/2602.11685"],
    });
    expect(wf.parameters).toMatchObject({
      engine: "exa",
      blockedDomains: ["arxiv.org/abs/2602.11685"],
    });
  });
  it("preserves a tool entry own known params; blocklist keys win on conflict; unknown keys dropped", () => {
    const cfg = config({
      tools: [
        {
          type: "openrouter:web_search",
          parameters: { engine: "perplexity", maxResults: 7, foo: 1 },
        },
      ],
    });
    const ws = experimentTools(cfg).find(isWebSearch)!;
    expect(ws.parameters).toMatchObject({
      engine: "exa",
      maxResults: 7,
      excludedDomains: ["arxiv.org/abs/2602.11685"],
    });
    expect("foo" in (ws.parameters ?? {})).toBe(false);
  });
  it("emits no blocklist params when engine is null and blockedDomains is empty", () => {
    const cfg = config({
      searchEngine: null,
      blockedDomains: [],
      tools: [{ type: "openrouter:web_search" }],
    });
    const ws = experimentTools(cfg).find(isWebSearch)!;
    expect(ws.parameters).toBeUndefined();
  });
  it("includes openrouter:shell in DEFAULT_TOOLS with engine + container_auto environment", () => {
    const tools = experimentTools(config());
    const shell = tools.find(isShell)!;
    expect(shell.type).toBe("openrouter:shell");
    expect(shell.parameters).toMatchObject({
      engine: "openrouter",
      environment: { type: "container_auto" },
    });
  });
  it("passes shell through untouched by the blocklist (no engine/domain injection)", () => {
    const cfg = config({
      searchEngine: "exa",
      blockedDomains: ["arxiv.org/abs/2602.11685"],
      tools: [
        {
          type: "openrouter:shell",
          parameters: {
            engine: "openrouter",
            environment: { type: "container_auto" },
          },
        },
      ],
    });
    const shell = experimentTools(cfg).find(isShell)!;
    expect(shell.parameters).toMatchObject({
      engine: "openrouter",
      environment: { type: "container_auto" },
    });
    expect("excludedDomains" in (shell.parameters ?? {})).toBe(false);
    expect("blockedDomains" in (shell.parameters ?? {})).toBe(false);
  });
});
describe("buildSoloBody", () => {
  it("builds a tooled solo body with instructions + tools + budget prefix", () => {
    const body = buildSoloBody("Q?", config());
    expect(body.model).toBe("openai/gpt-4o-mini");
    expect(body.instructions).toEqual(expect.any(String));
    expect(body.maxToolCalls).toBe(16);
    expect(body.maxOutputTokens).toBe(16384);
    expect(Array.isArray(body.tools)).toBe(true);
    const content = firstInputContent(body);
    expect(content).toContain("Q?");
    expect(content).toContain("budget of 16 tool calls");
  });
  it("builds a bare solo body (no instructions/tools) when tools is []", () => {
    const body = buildSoloBody("Q?", config({ tools: [] }));
    expect(body.instructions).toBeUndefined();
    expect(body.tools).toBeUndefined();
    expect(firstInputContent(body)).toBe("Q?");
  });
  it("forwards the provider routing block verbatim", () => {
    const body = buildSoloBody(
      "Q?",
      config({ provider: { only: ["DeepInfra"] } })
    );
    expect(body.provider).toEqual({ only: ["DeepInfra"] });
  });
});
describe("buildFusionBody", () => {
  it("targets openrouter/fusion and forwards synthesis + panel via the fusion tool parameters", () => {
    const body = buildFusionBody(
      "Q?",
      config({
        type: "fusion",
        synthesisModel: "anthropic/claude-opus-5",
        analysisModels: ["anthropic/claude-opus-5", "openai/gpt-5.5"],
      })
    );
    expect(body.model).toBe("openrouter/fusion");
    expect(body.instructions).toEqual(expect.stringContaining("Use Fusion."));
    const fusionTool = (body.tools ?? []).find(isFusion)!;
    const params = fusionTool.parameters ?? {};
    expect(params["model"]).toBe("anthropic/claude-opus-5");
    expect(params["analysisModels"]).toEqual([
      "anthropic/claude-opus-5",
      "openai/gpt-5.5",
    ]);
    expect(params["maxCompletionTokens"]).toBe(16384);
    expect("maxOutputTokens" in params).toBe(false);
    const panelTools = parseSchema(
      z.array(
        z.object({
          type: z.string(),
          parameters: z.record(z.string(), z.unknown()).optional(),
        })
      ),
      params["tools"] ?? []
    );
    assertRight(panelTools);
    const panelTypes = panelTools.right.map((t) => t.type).toSorted();
    expect(panelTypes).toEqual(
      [
        "openrouter:shell",
        "openrouter:web_fetch",
        "openrouter:web_search",
      ].toSorted()
    );
    const webSearch = panelTools.right.find(
      (t) => t.type === "openrouter:web_search"
    )!;
    expect(webSearch.parameters?.["engine"]).toBe("exa");
    expect(webSearch.parameters?.["excluded_domains"]).toEqual([
      "arxiv.org/abs/2602.11685",
    ]);
    expect(webSearch.parameters?.["excludedDomains"]).toBeUndefined();
    const shell = panelTools.right.find((t) => t.type === "openrouter:shell")!;
    expect(shell.parameters?.["environment"]).toEqual({
      type: "container_auto",
    });
  });
  it("forwards an explicit empty tools list to disable server-default panel tools", () => {
    const body = buildFusionBody("Q?", config({ type: "fusion", tools: [] }));
    const fusionTool = (body.tools ?? []).find(isFusion)!;
    const params = fusionTool.parameters ?? {};
    expect(params["tools"]).toEqual([]);
  });
  it("forwards provider routing through the fusion request", () => {
    const body = buildFusionBody(
      "Q?",
      config({
        type: "fusion",
        provider: { only: ["DeepInfra"], ignore: ["OpenAI"] },
      })
    );
    expect(body.provider).toEqual({ only: ["DeepInfra"], ignore: ["OpenAI"] });
  });
});
