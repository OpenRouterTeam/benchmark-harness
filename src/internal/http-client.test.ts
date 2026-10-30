import { afterEach, describe, expect, it } from "bun:test";

import { flatMap, provide, runPromise } from "effect/Effect";
import { HttpClient } from "effect/http";

import { fetchHttpClientLayer } from "./http-client";

const originalFetch = globalThis.fetch;

function stubFetch(body: string): typeof fetch {
  return (() =>
    Promise.resolve(new Response(body, { status: 200 }))) as typeof fetch;
}

async function getText(
  url: string,
  headers: Record<string, string> = {}
): Promise<string> {
  return runPromise(
    HttpClient.HttpClient.pipe(
      flatMap((client) => client.get(url, { headers })),
      flatMap((response) => response.text),
      provide(fetchHttpClientLayer)
    )
  );
}

describe("fetchHttpClientLayer", () => {
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("refuses redirects only for credential-bearing requests", async () => {
    let redirect: RequestRedirect | undefined;
    globalThis.fetch = ((
      _input: string | URL | Request,
      init?: RequestInit
    ) => {
      redirect = init?.redirect;
      return Promise.resolve(new Response("ok", { status: 200 }));
    }) as typeof fetch;
    await getText("https://example.invalid/a");
    expect(redirect).toBeUndefined();
    await getText("https://example.invalid/b", { Authorization: "Bearer k" });
    expect(redirect).toBe("error");
    await getText("https://example.invalid/c", { "CF-Access-Client-Id": "id" });
    expect(redirect).toBe("error");
  });

  it("resolves globalThis.fetch per request instead of once per process", async () => {
    globalThis.fetch = stubFetch("first");
    expect(await getText("https://example.invalid/a")).toBe("first");
    globalThis.fetch = stubFetch("second");
    expect(await getText("https://example.invalid/b")).toBe("second");
  });
});
