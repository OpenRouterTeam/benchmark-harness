const SANDBOX_AGENT_ENDPOINT = "https://openrouter.ai/api/v1";

const ACCESS_HEADER_PREFIX = "cf-access-";

function normalizedEndpoint(baseUrl: string): string | undefined {
  if (!URL.canParse(baseUrl)) {
    return undefined;
  }
  const url = new URL(baseUrl);
  return `${url.origin}${url.pathname.replace(/\/+$/u, "")}`;
}

export function sandboxAgentTargetError(opts: {
  readonly benchmarkId: string;
  readonly agent: string;
  readonly baseUrl: string | undefined;
  readonly traceHeaders: Readonly<Record<string, string>> | undefined;
}): Error | undefined {
  if (
    opts.baseUrl !== undefined &&
    normalizedEndpoint(opts.baseUrl) !== SANDBOX_AGENT_ENDPOINT
  ) {
    return new Error(
      `${opts.benchmarkId} cannot target a custom base URL with the ${opts.agent} agent: it calls the model from inside the sandbox, which only reaches ${SANDBOX_AGENT_ENDPOINT}`
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
