import { afterEach, describe, expect, it } from "bun:test";

import { flatMap, provideService, runPromise, succeed } from "effect/Effect";

import { recordGenerationId, resetGenerationIds } from "./generation-ids";
import type { GenerationResolverService } from "./generation-resolver";
import {
  GenerationResolver,
  makeOpenRouterGenerationResolver,
  resolveCollectedGenerationIds,
} from "./generation-resolver";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function mockFetch(
  handler: (url: string) => Response
): () => readonly string[] {
  const calls: string[] = [];
  globalThis.fetch = ((input: string | URL | Request) => {
    const url = typeof input === "string" ? input : new Request(input).url;
    calls.push(url);
    return Promise.resolve(handler(url));
  }) as typeof fetch;
  return () => calls;
}

describe("resolveCollectedGenerationIds", () => {
  it("returns collected ids unchanged when no resolver is provided", async () => {
    const ids = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-real")),
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => resolveCollectedGenerationIds)
      )
    );
    expect([...ids].toSorted()).toEqual(["gen-dummy", "gen-real"]);
  });
  it("resolves only cache-hit ids through the resolver", async () => {
    const resolved: string[] = [];
    const resolver: GenerationResolverService = {
      resolveSourceGenerationId: (generationId) => {
        resolved.push(generationId);
        return succeed(`source-of-${generationId}`);
      },
    };
    const ids = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-real")),
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => resolveCollectedGenerationIds),
        provideService(GenerationResolver, resolver)
      )
    );
    expect(resolved).toEqual(["gen-dummy"]);
    expect([...ids].toSorted()).toEqual(["gen-real", "source-of-gen-dummy"]);
  });
  it("keeps the dummy id when resolution returns undefined", async () => {
    const resolver: GenerationResolverService = {
      resolveSourceGenerationId: () => succeed(undefined),
    };
    const ids = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => resolveCollectedGenerationIds),
        provideService(GenerationResolver, resolver)
      )
    );
    expect(ids).toEqual(["gen-dummy"]);
  });
});

describe("makeOpenRouterGenerationResolver", () => {
  it("resolves the source generation id from the generation endpoint", async () => {
    const getCalls = mockFetch(() =>
      jsonResponse({ data: { response_cache_source_id: "gen-original" } })
    );
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 1,
    });
    const sourceId = await runPromise(
      resolver.resolveSourceGenerationId("gen-dummy")
    );
    expect(sourceId).toBe("gen-original");
    expect(getCalls()).toEqual([
      "https://example.com/api/v1/generation?id=gen-dummy",
    ]);
  });
  it("polls until the generation row lands", async () => {
    let attempts = 0;
    const getCalls = mockFetch(() => {
      attempts += 1;
      return attempts < 3
        ? jsonResponse({ error: "not found" }, 404)
        : jsonResponse({ data: { response_cache_source_id: "gen-original" } });
    });
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 5,
    });
    const sourceId = await runPromise(
      resolver.resolveSourceGenerationId("gen-dummy")
    );
    expect(sourceId).toBe("gen-original");
    expect(getCalls().length).toBe(3);
  });
  it("returns undefined when the source id never becomes available", async () => {
    mockFetch(() => jsonResponse({ data: { response_cache_source_id: null } }));
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 2,
    });
    const sourceId = await runPromise(
      resolver.resolveSourceGenerationId("gen-dummy")
    );
    expect(sourceId).toBeUndefined();
  });
});
