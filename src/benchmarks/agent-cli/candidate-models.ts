import type { SwitchyardAlgorithm } from "../../harness/constants";

export function sandboxAgentCandidateModelsError(opts: {
  readonly benchmarkId: string;
  readonly agent: string;
  readonly models: readonly string[] | undefined;
  readonly switchyardAlgorithm: SwitchyardAlgorithm | undefined;
}): Error | undefined {
  if (opts.models !== undefined) {
    return new Error(
      `${opts.benchmarkId} cannot use candidate models: the ${opts.agent} agent calls the model from inside the sandbox, which does not forward the models list`
    );
  }
  if (opts.switchyardAlgorithm !== undefined) {
    return new Error(
      `${opts.benchmarkId} cannot use switchyardAlgorithm: the ${opts.agent} agent calls the model from inside the sandbox, which does not forward the switchyard-router plugin`
    );
  }
  return undefined;
}
