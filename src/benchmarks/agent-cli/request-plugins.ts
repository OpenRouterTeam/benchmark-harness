import { stripVariantSuffix } from "../../harness/constants";
import type { GenerateConfig } from "../../harness/model";
import {
  buildAutoRouterPlugin,
  toWireAutoRouterPlugin,
} from "../../providers/auto-router-plugin";
import type { OriHarnessDef } from "./harness";
import { ORI_HARNESSES } from "./harness";
import type { AgentRequestPlugin } from "./runner";

export function sandboxAgentRequestPlugins(opts: {
  readonly benchmarkId: string;
  readonly harness: OriHarnessDef;
  readonly model: string;
  readonly costTier: GenerateConfig["costTier"];
  readonly costQualityTradeoff: number | undefined;
  readonly pinModel: boolean | undefined;
}): readonly AgentRequestPlugin[] | Error {
  const plugin = buildAutoRouterPlugin(stripVariantSuffix(opts.model), {
    costTier: opts.costTier,
    costQualityTradeoff: opts.costQualityTradeoff,
    pinModel: opts.pinModel,
  });
  if (plugin === undefined) {
    return [];
  }
  if (!opts.harness.forwardsRequestPlugins) {
    const fields = [
      opts.costTier !== undefined ? "costTier" : undefined,
      opts.costQualityTradeoff !== undefined
        ? "costQualityTradeoff"
        : undefined,
      opts.pinModel === true ? "pinModel" : undefined,
    ].filter((field) => field !== undefined);
    const forwardingAgents = Object.values(ORI_HARNESSES)
      .filter((harness) => harness.forwardsRequestPlugins)
      .map((harness) => harness.id);
    return new Error(
      `${opts.benchmarkId} cannot use ${fields.join(", ")}: the ${opts.harness.id} agent calls the model from inside the sandbox and does not forward the ${plugin.id} plugin (supported agents: ${forwardingAgents.join(", ")})`
    );
  }
  return [toWireAutoRouterPlugin(plugin)];
}
