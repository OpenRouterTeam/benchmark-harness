import { describe, expect, it } from "bun:test";

import { sandboxAgentTargetError } from "./sandbox-target";

const BASE = { benchmarkId: "deep_swe", agent: "pi" } as const;

describe("sandboxAgentTargetError", () => {
  it("accepts the default OpenRouter origin without Access headers", () => {
    expect(
      sandboxAgentTargetError({
        ...BASE,
        baseUrl: "https://openrouter.ai/api/v1",
        traceHeaders: { "X-OpenRouter-Experiment-Ids": "exp" },
      })
    ).toBeUndefined();
    expect(
      sandboxAgentTargetError({
        ...BASE,
        baseUrl: undefined,
        traceHeaders: undefined,
      })
    ).toBeUndefined();
  });

  it("rejects a same-origin URL on a different path and a malformed URL", () => {
    for (const baseUrl of [
      "https://openrouter.ai/preview/api/v1",
      "https://openrouter.ai/",
      "not-a-url",
    ]) {
      expect(
        sandboxAgentTargetError({ ...BASE, baseUrl, traceHeaders: undefined })
          ?.message
      ).toContain("cannot target a custom base URL");
    }
  });

  it("accepts the default endpoint with a trailing slash", () => {
    expect(
      sandboxAgentTargetError({
        ...BASE,
        baseUrl: "https://openrouter.ai/api/v1/",
        traceHeaders: undefined,
      })
    ).toBeUndefined();
  });

  it("rejects an api-perf Preview base URL", () => {
    const error = sandboxAgentTargetError({
      ...BASE,
      baseUrl: "https://abc123-api-perf.openrouter.workers.dev/api/v1",
      traceHeaders: undefined,
    });
    expect(error?.message).toContain("cannot target a custom base URL");
  });

  it("rejects Cloudflare Access headers", () => {
    const error = sandboxAgentTargetError({
      ...BASE,
      baseUrl: undefined,
      traceHeaders: { "CF-Access-Client-Id": "id" },
    });
    expect(error?.message).toContain("Cloudflare Access headers");
  });
});
