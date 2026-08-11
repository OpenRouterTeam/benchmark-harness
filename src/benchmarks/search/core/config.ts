import {
  MAX_SERVER_TOOL_CALLS,
  WEB_SEARCH_MAX_CHARACTERS_MAX,
  WEB_SEARCH_MAX_RESULTS_MAX,
  WebFetchEngine,
  WebSearchEngine,
} from "../../../internal/enums";
import type { ValueOf } from "../../../internal/guards";
import { z } from "../../../internal/zod";

export const SearchEngineSchema = z.union([
  z.literal("auto"),
  z.nativeEnum(WebSearchEngine),
]);

export type SearchEngine = z.infer<typeof SearchEngineSchema>;

export const FetchEngineSchema = z.union([
  z.literal("auto"),
  z.literal("native"),
  z.nativeEnum(WebFetchEngine),
]);

export type FetchEngine = z.infer<typeof FetchEngineSchema>;

export const SEARCH_SURFACES = ["server-tool", "plugin"] as const;

export type SearchSurface = ValueOf<typeof SEARCH_SURFACES>;

export const SEARCH_CONTEXT_SIZES = ["low", "medium", "high"] as const;

export type SearchContextSize = ValueOf<typeof SEARCH_CONTEXT_SIZES>;

export const WebFetchConfigSchema = z.object({
  fetchEngine: FetchEngineSchema.optional(),
  maxFetchUses: z.number().int().min(1).optional(),
  maxFetchContentTokens: z.number().int().min(1).optional(),
});

export type WebFetchConfig = z.infer<typeof WebFetchConfigSchema>;

export const SearchLaneConfigSchema = z.object({
  webSearch: z.enum(SEARCH_SURFACES).default("server-tool"),
  engine: SearchEngineSchema.default("auto"),
  maxAgentTurns: z.number().int().min(1).max(MAX_SERVER_TOOL_CALLS).optional(),
  maxResults: z
    .number()
    .int()
    .min(1)
    .max(WEB_SEARCH_MAX_RESULTS_MAX)
    .optional(),
  maxTotalResults: z.number().int().min(1).optional(),
  searchContextSize: z.enum(SEARCH_CONTEXT_SIZES).optional(),
  maxCharacters: z
    .number()
    .int()
    .min(1)
    .max(WEB_SEARCH_MAX_CHARACTERS_MAX)
    .optional(),
  allowedDomains: z.array(z.string()).optional(),
  excludedDomains: z.array(z.string()).optional(),
  searchPrompt: z.string().optional(),
  providerFlags: z.array(z.string().min(1)).optional(),
  webFetch: WebFetchConfigSchema.optional(),
});

export type SearchLaneConfig = z.infer<typeof SearchLaneConfigSchema>;
