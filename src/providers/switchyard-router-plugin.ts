import type { SwitchyardAlgorithm } from "../harness/constants";

export const SWITCHYARD_MODEL = "nvidia/switchyard";

export interface SwitchyardRouterPluginConfig {
  readonly id: "switchyard-router";
  readonly algorithm: SwitchyardAlgorithm;
}

export function buildSwitchyardRouterPlugin(
  baseModel: string | undefined,
  algorithm: SwitchyardAlgorithm | undefined
): SwitchyardRouterPluginConfig | undefined {
  if (baseModel !== SWITCHYARD_MODEL || algorithm === undefined) {
    return undefined;
  }
  return { id: "switchyard-router", algorithm };
}
