import type { SwitchyardAlgorithm } from "../../harness/constants";
import type { GenerateConfig } from "../../harness/model";
import { buildRouterPlugin } from "../../providers/router-plugin";
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
  readonly switchyardAlgorithm?: SwitchyardAlgorithm | undefined;
}): readonly AgentRequestPlugin[] | Error {
  const plugin = buildRouterPlugin(opts.model, opts);
  if (plugin === undefined) {
    return [];
  }
  const plugins = [{ ...plugin }];
  if (opts.harness.forwardsRequestPlugins) {
    return plugins;
  }
  const isAutoRouter =
    plugin.id === "auto-router" || plugin.id === "auto-beta-router";
  const fields = [
    plugin.id !== "switchyard-router" && opts.costTier !== undefined
      ? "costTier"
      : undefined,
    isAutoRouter && opts.costQualityTradeoff !== undefined
      ? "costQualityTradeoff"
      : undefined,
    isAutoRouter && opts.pinModel === true ? "pinModel" : undefined,
    plugin.id === "switchyard-router" ? "switchyardAlgorithm" : undefined,
  ].filter((field) => field !== undefined);
  const forwardingAgents = Object.values(ORI_HARNESSES)
    .filter((harness) => harness.forwardsRequestPlugins)
    .map((harness) => harness.id);
  return new Error(
    `${opts.benchmarkId} cannot use ${fields.join(", ")}: the ${opts.harness.id} agent calls the model from inside the sandbox and does not forward the ${plugins.map((plugin) => plugin.id).join(", ")} plugin (supported agents: ${forwardingAgents.join(", ")})`
  );
}
