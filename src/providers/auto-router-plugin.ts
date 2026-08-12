import type { GenerateConfig } from "../harness/model";

interface AutoRouterPluginOptions {
  readonly costTier?: GenerateConfig["costTier"];
  readonly costQualityTradeoff?: number;
  readonly pinModel?: boolean;
}

export interface AutoRouterPluginConfig {
  readonly id: "auto-router" | "auto-beta-router";
  readonly costTier?: GenerateConfig["costTier"];
  readonly costQualityTradeoff?: number;
  readonly pinModel?: boolean;
}

export function buildAutoRouterPlugin(
  baseModel: string | undefined,
  options: AutoRouterPluginOptions
): AutoRouterPluginConfig | undefined {
  let id: AutoRouterPluginConfig["id"] | undefined;
  if (baseModel === "openrouter/auto") {
    id = "auto-router";
  } else if (baseModel === "openrouter/auto-beta") {
    id = "auto-beta-router";
  }
  if (
    id === undefined ||
    (options.costTier === undefined &&
      options.costQualityTradeoff === undefined &&
      options.pinModel !== true)
  ) {
    return undefined;
  }
  return {
    id,
    ...(options.costTier !== undefined && { costTier: options.costTier }),
    ...(options.costQualityTradeoff !== undefined && {
      costQualityTradeoff: options.costQualityTradeoff,
    }),
    ...(options.pinModel === true && { pinModel: true }),
  };
}

export function toWireAutoRouterPlugin(plugin: AutoRouterPluginConfig): {
  readonly id: AutoRouterPluginConfig["id"];
  readonly cost_tier?: GenerateConfig["costTier"];
  readonly cost_quality_tradeoff?: number;
  readonly pin_model?: boolean;
} {
  return {
    id: plugin.id,
    ...(plugin.costTier !== undefined && { cost_tier: plugin.costTier }),
    ...(plugin.costQualityTradeoff !== undefined && {
      cost_quality_tradeoff: plugin.costQualityTradeoff,
    }),
    ...(plugin.pinModel === true && { pin_model: true }),
  };
}
