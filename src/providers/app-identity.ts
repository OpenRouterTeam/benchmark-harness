export const BENCH_HARNESS_APP_REFERRER =
  "https://bench-harness.openrouter.ai/";

export const BENCH_HARNESS_APP_TITLE = "OpenRouter: Bench Harness";

export const REQUESTED_BY_ENV = "OPENROUTER_REQUESTED_BY";
export const REQUESTED_BY_HEADER = "X-OpenRouter-Requested-By";

export function requestedByHeaders(
  env: Readonly<Record<string, string | undefined>> = process.env
): Record<string, string> {
  const requestedBy = env[REQUESTED_BY_ENV];
  return requestedBy ? { [REQUESTED_BY_HEADER]: requestedBy } : {};
}
