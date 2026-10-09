import type { CostTier } from "../harness/constants";
import { stripVariantSuffix } from "../harness/constants";
import type { GenerateConfig } from "../harness/model";
import { definedValues } from "../internal/guards";
import type { SwitchyardRouterPluginConfig } from "./switchyard-router-plugin";
import { buildSwitchyardRouterPlugin } from "./switchyard-router-plugin";

type RouterPluginConfig =
  | {
      readonly id: "auto-router" | "auto-beta-router";
      readonly cost_tier?: CostTier;
      readonly cost_quality_tradeoff?: number;
      readonly pin_model?: boolean;
    }
  | { readonly id: "jev-router"; readonly cost_tier: CostTier }
  | SwitchyardRouterPluginConfig;

export function buildRouterPlugin(
  model: string | undefined,
  options: Pick<
    GenerateConfig,
    "costTier" | "costQualityTradeoff" | "pinModel" | "switchyardAlgorithm"
  >
): RouterPluginConfig | undefined {
  const baseModel = model === undefined ? undefined : stripVariantSuffix(model);
  switch (baseModel) {
    case "typesafe/jev-router": {
      return options.costTier === undefined
        ? undefined
        : { id: "jev-router", cost_tier: options.costTier };
    }
    case "openrouter/auto":
    case "openrouter/auto-beta": {
      if (
        options.costTier === undefined &&
        options.costQualityTradeoff === undefined &&
        options.pinModel !== true
      ) {
        return undefined;
      }
      return definedValues({
        id:
          baseModel === "openrouter/auto" ? "auto-router" : "auto-beta-router",
        cost_tier: options.costTier,
        cost_quality_tradeoff: options.costQualityTradeoff,
        pin_model: options.pinModel === true ? true : undefined,
      } satisfies RouterPluginConfig);
    }
    default: {
      return buildSwitchyardRouterPlugin(
        baseModel,
        options.switchyardAlgorithm
      );
    }
  }
}
