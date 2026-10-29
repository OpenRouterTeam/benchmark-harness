import { afterEach, describe, expect, it } from "bun:test";

import { flatMap, provide, runPromise } from "effect/Effect";
import { HttpClient } from "effect/http";

import { fetchHttpClientLayer } from "./http-client";

const originalFetch = globalThis.fetch;

function stubFetch(body: string): typeof fetch {
  const stub = (() =>
    Promise.resolve(new Response(body, { status: 200 }))) as typeof fetch;
  return stub;
}

async function getText(url: string): Promise<string> {
  return runPromise(
    HttpClient.HttpClient.pipe(
      flatMap((client) => client.get(url)),
      flatMap((response) => response.text),
      provide(fetchHttpClientLayer)
    )
  );
}

describe("fetchHttpClientLayer", () => {
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it("resolves globalThis.fetch per request instead of once per process", async () => {
    globalThis.fetch = stubFetch("first");
    expect(await getText("https://example.invalid/a")).toBe("first");
    globalThis.fetch = stubFetch("second");
    expect(await getText("https://example.invalid/b")).toBe("second");
  });
});
