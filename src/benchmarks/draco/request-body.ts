import type {
  FusionServerToolConfig,
  FusionServerToolConfigTool,
  FusionServerToolOpenRouter,
  ResponsesRequest,
  ResponsesRequestToolUnion,
  ShellServerToolConfig,
  ShellServerToolOpenRouter,
  WebFetchServerTool,
  WebFetchServerToolConfig,
  WebSearchServerToolConfig,
  WebSearchServerToolOpenRouter,
} from "@openrouter/sdk/models";
import { snakeCase } from "change-case";
import { option, string } from "effect/Config";
import { runSync } from "effect/Effect";
import { getOrNull } from "effect/Option";

import type { ValueOf } from "../../internal/guards";
import { definedValues, isRecord } from "../../internal/guards";
import { responsesMessage } from "../../providers/responses-model";
import {
  AGENT_SYSTEM_PROMPT,
  FUSION_CLASSIFIER_DIRECTIVE,
  buildInputPrefix,
} from "./prompts";
import type {
  DracoPanelConfig,
  ShellToolParameters,
  ToolEntry,
} from "./schemas";
import { DracoToolType } from "./schemas";

function resolveMaxToolCalls(): number {
  const v = getOrNull(runSync(string("BENCHMARK_MAX_TOOL_CALLS").pipe(option)));
  return v === null ? 16 : Number(v);
}

export const MAX_TOOL_CALLS = resolveMaxToolCalls();

export const MAX_OUTPUT_TOKENS = 16384;

const WEB_TOOL_DOMAIN_KEY: Partial<
  Readonly<Record<DracoToolType, "excludedDomains" | "blockedDomains">>
> = {
  [DracoToolType.WebSearch]: "excludedDomains",
  [DracoToolType.WebFetch]: "blockedDomains",
};

export const DEFAULT_TOOLS: readonly ToolEntry[] = [
  { type: DracoToolType.WebSearch },
  { type: DracoToolType.WebFetch },
  {
    type: DracoToolType.Shell,
    parameters: {
      engine: "openrouter",
      environment: { type: "container_auto" },
    },
  },
];

function blocklistParams(
  config: DracoPanelConfig,
  domainKey: "excludedDomains" | "blockedDomains" | undefined
): Record<string, unknown> {
  if (domainKey === undefined) {
    return {};
  }
  const params: Record<string, unknown> = {};
  if (config.searchEngine) {
    params["engine"] = config.searchEngine;
  }
  if (config.blockedDomains.length > 0) {
    params[domainKey] = [...config.blockedDomains];
  }
  return params;
}

export function applyBlocklist(
  tools: readonly ToolEntry[],
  config: DracoPanelConfig
): ToolEntry[] {
  const out: ToolEntry[] = [];
  for (const tool of tools) {
    const domainKey = WEB_TOOL_DOMAIN_KEY[tool.type];
    const block = blocklistParams(config, domainKey);
    if (Object.keys(block).length === 0) {
      out.push(tool);
      continue;
    }
    out.push({
      ...tool,
      parameters: { ...tool.parameters, ...block },
    });
  }
  return out;
}

export function experimentTools(
  config: DracoPanelConfig
): ResponsesRequestToolUnion[] {
  return applyBlocklist(config.tools ?? DEFAULT_TOOLS, config).map(
    toRequestTool
  );
}

const SEARCH_ENGINES = [
  "auto",
  "native",
  "exa",
  "parallel",
  "firecrawl",
] as const;

type SearchEngine = ValueOf<typeof SEARCH_ENGINES>;

const SEARCH_ENGINE_LIST: readonly string[] = SEARCH_ENGINES;

function isSearchEngine(value: unknown): value is SearchEngine {
  return typeof value === "string" && SEARCH_ENGINE_LIST.includes(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((x) => typeof x === "string");
}

function toSearchParams(
  params: Record<string, unknown> | undefined
): WebSearchServerToolConfig | undefined {
  if (params === undefined) {
    return undefined;
  }
  const config: WebSearchServerToolConfig = {};
  if (isSearchEngine(params["engine"])) {
    config.engine = params["engine"];
  }
  if (isStringArray(params["allowedDomains"])) {
    config.allowedDomains = params["allowedDomains"];
  }
  if (isStringArray(params["excludedDomains"])) {
    config.excludedDomains = params["excludedDomains"];
  }
  if (typeof params["maxResults"] === "number") {
    config.maxResults = params["maxResults"];
  }
  if (typeof params["maxTotalResults"] === "number") {
    config.maxTotalResults = params["maxTotalResults"];
  }
  const ctx = params["searchContextSize"];
  if (ctx === "low" || ctx === "medium" || ctx === "high") {
    config.searchContextSize = ctx;
  }
  return Object.keys(config).length > 0 ? config : undefined;
}

function toFetchParams(
  params: Record<string, unknown> | undefined
): WebFetchServerToolConfig | undefined {
  if (params === undefined) {
    return undefined;
  }
  const config: WebFetchServerToolConfig = {};
  if (isSearchEngine(params["engine"])) {
    config.engine = params["engine"];
  }
  if (isStringArray(params["allowedDomains"])) {
    config.allowedDomains = params["allowedDomains"];
  }
  if (isStringArray(params["blockedDomains"])) {
    config.blockedDomains = params["blockedDomains"];
  }
  if (typeof params["maxContentTokens"] === "number") {
    config.maxContentTokens = params["maxContentTokens"];
  }
  if (typeof params["maxUses"] === "number") {
    config.maxUses = params["maxUses"];
  }
  return Object.keys(config).length > 0 ? config : undefined;
}

function toShellParams(
  parameters: ShellToolParameters | undefined
): ShellServerToolConfig | undefined {
  if (parameters === undefined) {
    return undefined;
  }
  const config: ShellServerToolConfig = {};
  if (parameters.engine !== undefined) {
    config.engine = parameters.engine;
  }
  if (parameters.environment !== undefined) {
    config.environment = parameters.environment;
  }
  if (parameters.sleepAfterSeconds !== undefined) {
    config.sleepAfterSeconds = parameters.sleepAfterSeconds;
  }
  return Object.keys(config).length > 0 ? config : undefined;
}

function toRequestTool(tool: ToolEntry): ResponsesRequestToolUnion {
  switch (tool.type) {
    case DracoToolType.WebSearch: {
      const parameters = toSearchParams(tool.parameters);
      const webSearch: WebSearchServerToolOpenRouter = definedValues({
        type: DracoToolType.WebSearch,
        parameters,
      });
      return webSearch;
    }
    case DracoToolType.WebFetch: {
      const parameters = toFetchParams(tool.parameters);
      const webFetch: WebFetchServerTool = definedValues({
        type: DracoToolType.WebFetch,
        parameters,
      });
      return webFetch;
    }
    case DracoToolType.Shell: {
      const parameters = toShellParams(tool.parameters);
      const shell: ShellServerToolOpenRouter = definedValues({
        type: DracoToolType.Shell,
        parameters,
      });
      return shell;
    }
    default: {
      const _exhaustive: never = tool;
      return _exhaustive;
    }
  }
}

function toFusionPanelTool(tool: ToolEntry): FusionServerToolConfigTool {
  switch (tool.type) {
    case DracoToolType.WebSearch: {
      const parameters = toSearchParams(tool.parameters);
      return {
        type: DracoToolType.WebSearch,
        ...(parameters !== undefined && {
          parameters: deepSnakeCaseKeys(parameters),
        }),
      };
    }
    case DracoToolType.WebFetch: {
      const parameters = toFetchParams(tool.parameters);
      return {
        type: DracoToolType.WebFetch,
        ...(parameters !== undefined && {
          parameters: deepSnakeCaseKeys(parameters),
        }),
      };
    }
    case DracoToolType.Shell: {
      const parameters = toShellParams(tool.parameters);
      return {
        type: DracoToolType.Shell,
        ...(parameters !== undefined && {
          parameters: deepSnakeCaseKeys(parameters),
        }),
      };
    }
    default: {
      const _exhaustive: never = tool;
      return _exhaustive;
    }
  }
}

function deepSnakeCaseKeys(
  input: Record<string, unknown>
): Record<string, unknown> {
  const transform = (value: unknown): unknown => {
    if (Array.isArray(value)) {
      return value.map(transform);
    }
    if (isRecord(value)) {
      return Object.fromEntries(
        Object.entries(value).map(([k, v]) => [snakeCase(k), transform(v)])
      );
    }
    return value;
  };
  return Object.fromEntries(
    Object.entries(input).map(([k, v]) => [snakeCase(k), transform(v)])
  );
}

export function buildFusionBody(
  problem: string,
  config: DracoPanelConfig
): ResponsesRequest {
  const panelTools: FusionServerToolConfigTool[] = applyBlocklist(
    config.tools ?? DEFAULT_TOOLS,
    config
  ).map(toFusionPanelTool);
  const fusionParameters: FusionServerToolConfig = definedValues({
    maxToolCalls: MAX_TOOL_CALLS,
    maxCompletionTokens: MAX_OUTPUT_TOKENS,
    model: config.synthesisModel,
    ...(config.analysisModels.length > 0 && {
      analysisModels: [...config.analysisModels],
    }),
    ...((config.tools !== undefined || panelTools.length > 0) && {
      tools: panelTools,
    }),
  });
  const fusionTool: FusionServerToolOpenRouter = {
    type: "openrouter:fusion",
    parameters: fusionParameters,
  };
  return {
    model: "openrouter/fusion",
    instructions: FUSION_CLASSIFIER_DIRECTIVE + AGENT_SYSTEM_PROMPT,
    input: [
      responsesMessage("user", buildInputPrefix(MAX_TOOL_CALLS) + problem),
    ],
    maxToolCalls: MAX_TOOL_CALLS,
    maxOutputTokens: MAX_OUTPUT_TOKENS,
    tools: [fusionTool],
    ...(config.provider !== undefined && { provider: config.provider }),
  };
}

export function buildSoloBody(
  problem: string,
  config: DracoPanelConfig
): ResponsesRequest {
  const tools = experimentTools(config);
  const hasTools = tools.length > 0;
  const input = hasTools ? buildInputPrefix(MAX_TOOL_CALLS) + problem : problem;
  return definedValues({
    model: config.model,
    input: [responsesMessage("user", input)],
    maxToolCalls: MAX_TOOL_CALLS,
    maxOutputTokens: MAX_OUTPUT_TOKENS,
    ...(hasTools && { instructions: AGENT_SYSTEM_PROMPT, tools }),
    provider: config.provider,
  });
}
