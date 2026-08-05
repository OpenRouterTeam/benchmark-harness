import type { ValueOf } from "./guards";

export enum ProviderSort {
  Price = "price",
  Throughput = "throughput",
  Latency = "latency",
  Exacto = "exacto",
}

export const WebFetchEngine = {
  OpenRouter: "openrouter",
  Firecrawl: "firecrawl",
  Exa: "exa",
  Parallel: "parallel",
} as const;

export type WebFetchEngine = ValueOf<typeof WebFetchEngine>;

export enum WebSearchEngine {
  Native = "native",
  Exa = "exa",
  Firecrawl = "firecrawl",
  Parallel = "parallel",
  Perplexity = "perplexity",
}

export const MAX_SERVER_TOOL_CALLS = 30;

export const WEB_SEARCH_MAX_RESULTS_MAX = 25;

export const WEB_SEARCH_MAX_CHARACTERS_MAX = 100000;
