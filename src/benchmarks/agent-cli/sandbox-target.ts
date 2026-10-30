const OPENROUTER_ORIGIN = "https://openrouter.ai";

const ACCESS_HEADER_PREFIX = "cf-access-";

export function sandboxAgentTargetError(opts: {
  readonly benchmarkId: string;
  readonly agent: string;
  readonly baseUrl: string | undefined;
  readonly traceHeaders: Readonly<Record<string, string>> | undefined;
}): Error | undefined {
  const origin =
    opts.baseUrl !== undefined && URL.canParse(opts.baseUrl)
      ? new URL(opts.baseUrl).origin
      : undefined;
  if (origin !== undefined && origin !== OPENROUTER_ORIGIN) {
    return new Error(
      `${opts.benchmarkId} cannot target a custom base URL with the ${opts.agent} agent: it calls the model from inside the sandbox, which only reaches ${OPENROUTER_ORIGIN}`
    );
  }
  const carriesAccessToken = Object.keys(opts.traceHeaders ?? {}).some((name) =>
    name.toLowerCase().startsWith(ACCESS_HEADER_PREFIX)
  );
  if (carriesAccessToken) {
    return new Error(
      `${opts.benchmarkId} cannot carry Cloudflare Access headers with the ${opts.agent} agent: the sandboxed CLI does not forward them`
    );
  }
  return undefined;
}
