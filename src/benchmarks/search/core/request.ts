import type {
  ProviderPreferences,
  ResponsesRequest,
  WebFetchServerTool,
  WebFetchServerToolConfig,
  WebSearchPlugin,
  WebSearchServerToolConfig,
  WebSearchServerToolOpenRouter,
} from "@openrouter/sdk/models";

import type { CostTier, ReasoningEffort } from "../../../harness/constants";
import type { ProviderSort } from "../../../internal/enums";
import { definedValues } from "../../../internal/guards";
import { buildAutoRouterPlugin } from "../../../providers/auto-router-plugin";
import { responsesMessage } from "../../../providers/responses-model";
import { BENCHMARK_LEAK_EXCLUDED_DOMAINS } from "./blocklist";
import type { SearchLaneConfig, WebFetchConfig } from "./config";

function resolveExcludedDomains(lane: SearchLaneConfig): readonly string[] {
  return lane.excludedDomains ?? BENCHMARK_LEAK_EXCLUDED_DOMAINS;
}

export interface SearchRequestOptions {
  readonly model: string;
  readonly instructions: string;
  readonly problem: string;
  readonly lane: SearchLaneConfig;
  readonly maxOutputTokens?: number;
  readonly temperature?: number;
  readonly reasoningEffort?: ReasoningEffort;
  readonly sort?: ProviderSort;
  readonly providerOrder?: readonly string[];
  readonly providerOnly?: readonly string[];
  readonly allowFallbacks?: boolean;
  readonly costQualityTradeoff?: number;
  readonly costTier?: CostTier;
}

function toSearchToolParams(
  lane: SearchLaneConfig
): WebSearchServerToolConfig | undefined {
  const params = definedValues({
    engine: lane.engine === "auto" ? undefined : lane.engine,
    maxResults: lane.maxResults,
    maxTotalResults: lane.maxTotalResults,
    searchContextSize: lane.searchContextSize,
    maxCharacters: lane.maxCharacters,
    allowedDomains:
      lane.allowedDomains === undefined ? undefined : [...lane.allowedDomains],
    excludedDomains: [...resolveExcludedDomains(lane)],
  }) satisfies WebSearchServerToolConfig;
  return Object.keys(params).length > 0 ? params : undefined;
}

function toFetchToolParams(
  fetch: WebFetchConfig,
  excludedDomains: readonly string[]
): WebFetchServerToolConfig | undefined {
  const params = definedValues({
    engine: fetch.fetchEngine,
    maxUses: fetch.maxFetchUses,
    maxContentTokens: fetch.maxFetchContentTokens,
    blockedDomains: [...excludedDomains],
  }) satisfies WebFetchServerToolConfig;
  return Object.keys(params).length > 0 ? params : undefined;
}

function buildServerTools(
  lane: SearchLaneConfig
): readonly (WebSearchServerToolOpenRouter | WebFetchServerTool)[] {
  const searchParameters = toSearchToolParams(lane);
  const searchTool: WebSearchServerToolOpenRouter = {
    type: "openrouter:web_search",
    ...(searchParameters !== undefined && { parameters: searchParameters }),
  };
  if (lane.webFetch === undefined) {
    return [searchTool];
  }
  const fetchParameters = toFetchToolParams(
    lane.webFetch,
    resolveExcludedDomains(lane)
  );
  const fetchTool: WebFetchServerTool = {
    type: "openrouter:web_fetch",
    ...(fetchParameters !== undefined && { parameters: fetchParameters }),
  };
  return [searchTool, fetchTool];
}

function buildWebPlugin(lane: SearchLaneConfig): WebSearchPlugin {
  return {
    id: "web",
    ...definedValues({
      engine: lane.engine === "auto" ? undefined : lane.engine,
      maxResults: lane.maxResults,
      searchPrompt: lane.searchPrompt,
      includeDomains:
        lane.allowedDomains === undefined
          ? undefined
          : [...lane.allowedDomains],
      excludeDomains: [...resolveExcludedDomains(lane)],
    }),
  };
}

export function buildSearchRequestBody(
  opts: SearchRequestOptions
): ResponsesRequest {
  const { lane } = opts;
  const autoRouterPlugin = buildAutoRouterPlugin(
    opts.model === "openrouter/auto" ? opts.model : undefined,
    opts
  );
  const base: ResponsesRequest = {
    model: opts.model,
    instructions: opts.instructions,
    input: [responsesMessage("user", opts.problem)],
    ...definedValues({
      maxOutputTokens: opts.maxOutputTokens,
      temperature: opts.temperature,
      ...(opts.reasoningEffort !== undefined && {
        reasoning: { effort: opts.reasoningEffort },
      }),
      provider:
        opts.sort !== undefined ||
        opts.providerOrder !== undefined ||
        opts.providerOnly !== undefined ||
        opts.allowFallbacks !== undefined
          ? (definedValues({
              sort: opts.sort,
              order:
                opts.providerOrder === undefined
                  ? undefined
                  : [...opts.providerOrder],
              only:
                opts.providerOnly === undefined
                  ? undefined
                  : [...opts.providerOnly],
              allowFallbacks: opts.allowFallbacks,
            }) satisfies ProviderPreferences)
          : undefined,
    }),
  };
  if (lane.webSearch === "plugin") {
    const webPlugins = [
      ...(autoRouterPlugin === undefined ? [] : [autoRouterPlugin]),
      buildWebPlugin(lane),
    ];
    return { ...base, plugins: webPlugins };
  }
  return {
    ...base,
    tools: [...buildServerTools(lane)],
    ...(lane.maxAgentTurns !== undefined && {
      maxToolCalls: lane.maxAgentTurns,
    }),
    ...(autoRouterPlugin !== undefined && { plugins: [autoRouterPlugin] }),
  };
}
