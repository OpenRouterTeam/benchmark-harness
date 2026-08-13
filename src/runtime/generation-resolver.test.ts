import { afterEach, describe, expect, it } from "bun:test";

import { flatMap, provideService, runPromise, succeed } from "effect/Effect";

import {
  recordGenerationId,
  resetGenerationIds,
  withAuxiliaryUsage,
} from "./generation-ids";
import type {
  GenerationResolverService,
  ReplayedUsage,
} from "./generation-resolver";
import {
  GenerationResolver,
  makeOpenRouterGenerationResolver,
  resolveCollectedGenerations,
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

const SOURCE_USAGE: ReplayedUsage = {
  inputTokens: 10,
  outputTokens: 25,
  totalTokens: 35,
  reasoningTokens: 5,
  totalCost: 0.0015,
  generationTimeMs: 1200,
};

describe("resolveCollectedGenerations", () => {
  it("returns collected ids unchanged when no resolver is provided", async () => {
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-real")),
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => resolveCollectedGenerations)
      )
    );
    expect([...resolved.ids].toSorted()).toEqual(["gen-dummy", "gen-real"]);
    expect(resolved.replayedUsage).toBeUndefined();
  });
  it("resolves only cache-hit ids through the resolver and sums replayed usage", async () => {
    const requested: string[] = [];
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: (generationId) => {
        requested.push(generationId);
        return succeed({
          sourceId: `source-of-${generationId}`,
          usage: SOURCE_USAGE,
        });
      },
    };
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-real")),
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => recordGenerationId("gen-dummy-2", true)),
        flatMap(() => resolveCollectedGenerations),
        provideService(GenerationResolver, resolver)
      )
    );
    expect([...requested].toSorted()).toEqual(["gen-dummy", "gen-dummy-2"]);
    expect([...resolved.ids].toSorted()).toEqual([
      "gen-real",
      "source-of-gen-dummy",
      "source-of-gen-dummy-2",
    ]);
    expect(resolved.replayedUsage).toEqual({
      inputTokens: 20,
      outputTokens: 50,
      totalTokens: 70,
      reasoningTokens: 10,
      totalCost: 0.003,
      generationTimeMs: 2400,
    });
  });
  it("resolves auxiliary cache-hit ids without counting their usage", async () => {
    const requested: { id: string; includeUsage: boolean | undefined }[] = [];
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: (generationId, options) => {
        requested.push({
          id: generationId,
          includeUsage: options?.includeUsage,
        });
        return succeed({
          sourceId: `source-of-${generationId}`,
          ...(options?.includeUsage === false ? {} : { usage: SOURCE_USAGE }),
        });
      },
    };
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-solver", true)),
        flatMap(() =>
          withAuxiliaryUsage(recordGenerationId("gen-judge", true))
        ),
        flatMap(() => resolveCollectedGenerations),
        provideService(GenerationResolver, resolver)
      )
    );
    expect([...requested].toSorted((a, b) => a.id.localeCompare(b.id))).toEqual(
      [
        { id: "gen-judge", includeUsage: false },
        { id: "gen-solver", includeUsage: true },
      ]
    );
    expect([...resolved.ids].toSorted()).toEqual([
      "source-of-gen-judge",
      "source-of-gen-solver",
    ]);
    expect(resolved.replayedUsage).toEqual(SOURCE_USAGE);
  });
  it("keeps the dummy id and reports no usage when resolution returns undefined", async () => {
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: () => succeed(undefined),
    };
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => resolveCollectedGenerations),
        provideService(GenerationResolver, resolver)
      )
    );
    expect(resolved.ids).toEqual(["gen-dummy"]);
    expect(resolved.replayedUsage).toBeUndefined();
  });
  it("omits usage for entries that resolve without usage", async () => {
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: (generationId) =>
        succeed({ sourceId: `source-of-${generationId}` }),
    };
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => resolveCollectedGenerations),
        provideService(GenerationResolver, resolver)
      )
    );
    expect(resolved.ids).toEqual(["source-of-gen-dummy"]);
    expect(resolved.replayedUsage).toBeUndefined();
  });
});

describe("makeOpenRouterGenerationResolver", () => {
  it("resolves the source generation id and fetches the source usage", async () => {
    const getCalls = mockFetch((url) =>
      url.includes("gen-dummy")
        ? jsonResponse({ data: { response_cache_source_id: "gen-original" } })
        : jsonResponse({
            data: {
              response_cache_source_id: null,
              tokens_prompt: 10,
              tokens_completion: 25,
              native_tokens_reasoning: 5,
              total_cost: 0.0015,
              generation_time: 1200,
            },
          })
    );
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 1,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-dummy")
    );
    expect(resolved).toEqual({
      sourceId: "gen-original",
      usage: SOURCE_USAGE,
    });
    expect(getCalls()).toEqual([
      "https://example.com/api/v1/generation?id=gen-dummy",
      "https://example.com/api/v1/generation?id=gen-original",
    ]);
  });
  it("skips the source usage lookup when includeUsage is false", async () => {
    const getCalls = mockFetch(() =>
      jsonResponse({ data: { response_cache_source_id: "gen-original" } })
    );
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 1,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-dummy", { includeUsage: false })
    );
    expect(resolved).toEqual({ sourceId: "gen-original" });
    expect(getCalls()).toEqual([
      "https://example.com/api/v1/generation?id=gen-dummy",
    ]);
  });
  it("polls until the generation row lands", async () => {
    let attempts = 0;
    const getCalls = mockFetch((url) => {
      if (url.includes("gen-original")) {
        return jsonResponse({ data: { response_cache_source_id: null } });
      }
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
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-dummy")
    );
    expect(resolved?.sourceId).toBe("gen-original");
    expect(getCalls().filter((url) => url.includes("gen-dummy")).length).toBe(
      3
    );
  });
  it("returns undefined after exactly maxAttempts lookups when the source id never becomes available", async () => {
    const getCalls = mockFetch(() =>
      jsonResponse({ data: { response_cache_source_id: null } })
    );
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 2,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-dummy")
    );
    expect(resolved).toBeUndefined();
    expect(getCalls().length).toBe(2);
  });
  it("returns the source id without usage when the source lookup fails", async () => {
    const getCalls = mockFetch((url) =>
      url.includes("gen-dummy")
        ? jsonResponse({ data: { response_cache_source_id: "gen-original" } })
        : jsonResponse({ error: "boom" }, 500)
    );
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 1,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-dummy")
    );
    expect(resolved).toEqual({ sourceId: "gen-original" });
    expect(getCalls().length).toBe(2);
  });
});
