import type { CostTier } from "../harness/constants";

export interface JevRouterPluginConfig {
  readonly id: "jev-router";
  readonly cost_tier: CostTier;
}

export function buildJevRouterPlugin(
  baseModel: string | undefined,
  costTier: CostTier | undefined
): JevRouterPluginConfig | undefined {
  if (baseModel !== "typesafe/jev-router" || costTier === undefined) {
    return undefined;
  }
  return { id: "jev-router", cost_tier: costTier };
}
