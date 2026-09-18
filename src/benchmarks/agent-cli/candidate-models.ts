export function sandboxAgentCandidateModelsError(opts: {
  readonly benchmarkId: string;
  readonly agent: string;
  readonly models: readonly string[] | undefined;
}): Error | undefined {
  if (opts.models === undefined) {
    return undefined;
  }
  return new Error(
    `${opts.benchmarkId} cannot use candidate models: the ${opts.agent} agent calls the model from inside the sandbox, which does not forward the models list`
  );
}
