import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";

import type { ProviderSort } from "../internal/enums";
import type { CostTier, ReasoningEffort } from "./constants";
import type {
  ChatMessage,
  ModelError,
  ModelOutput,
  ToolDefinition,
} from "./core";

export function stripVariantSuffix(model: string): string {
  const idx = model.indexOf(":");
  return idx <= 0 ? model : model.slice(0, idx);
}

export interface GenerateConfig {
  readonly temperature?: number;
  readonly maxTokens?: number;
  readonly endpointId?: string;
  readonly tools?: readonly ToolDefinition[];
  readonly reasoningEffort?: ReasoningEffort;
  readonly costTier?: CostTier;
  readonly timeoutMs?: number;
  readonly sort?: ProviderSort;
  readonly providerOnly?: readonly string[];
  readonly providerIgnore?: readonly string[];
  readonly allowFallbacks?: boolean;
  readonly cloudflareVersion?: string;
  readonly costQualityTradeoff?: number;
  readonly pinModel?: boolean;
  readonly extraBody?: Readonly<Record<string, unknown>>;
}

export class Model extends Tag("@openrouter/bench-harness/model")<
  Model,
  ModelService
>() {}

export interface ModelService {
  readonly generate: (
    messages: readonly ChatMessage[],
    config: GenerateConfig
  ) => Effect<ModelOutput, ModelError>;
}
