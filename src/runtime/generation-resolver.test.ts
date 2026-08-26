import { afterEach, describe, expect, it, spyOn } from "bun:test";

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
          sourceIds: [`source-of-${generationId}`],
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
          sourceIds: [`source-of-${generationId}`],
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
  it("skips resolution for auxiliary ids whose source is already resolved", async () => {
    const requested: string[] = [];
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: (generationId, options) => {
        requested.push(generationId);
        return succeed({
          sourceIds: [generationId],
          ...(options?.includeUsage === false ? {} : { usage: SOURCE_USAGE }),
        });
      },
    };
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-solver-source", true, true)),
        flatMap(() =>
          withAuxiliaryUsage(recordGenerationId("gen-judge-source", true, true))
        ),
        flatMap(() => resolveCollectedGenerations),
        provideService(GenerationResolver, resolver)
      )
    );
    expect(requested).toEqual(["gen-solver-source"]);
    expect([...resolved.ids].toSorted()).toEqual([
      "gen-judge-source",
      "gen-solver-source",
    ]);
    expect(resolved.replayedUsage).toEqual(SOURCE_USAGE);
  });
  it("omits usage for entries that resolve without usage", async () => {
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: (generationId) =>
        succeed({ sourceIds: [`source-of-${generationId}`] }),
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
  it("replaces marked server-tools roots with their child generation ids", async () => {
    const requested: {
      id: string;
      includeRelated: boolean | undefined;
    }[] = [];
    const resolver: GenerationResolverService = {
      resolveSourceGeneration: (generationId, options) => {
        requested.push({
          id: generationId,
          includeRelated: options?.includeRelated,
        });
        return succeed({ sourceIds: ["gen-child-1", "gen-child-2"] });
      },
    };
    const resolved = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-root", false, false, true)),
        flatMap(() => resolveCollectedGenerations),
        provideService(GenerationResolver, resolver)
      )
    );
    expect(requested).toEqual([{ id: "gen-root", includeRelated: true }]);
    expect(resolved.ids).toEqual(["gen-child-1", "gen-child-2"]);
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
              native_tokens_prompt: 10,
              native_tokens_completion: 25,
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
      sourceIds: ["gen-original"],
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
    expect(resolved).toEqual({ sourceIds: ["gen-original"] });
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
    expect(resolved?.sourceIds).toEqual(["gen-original"]);
    expect(getCalls().filter((url) => url.includes("gen-dummy")).length).toBe(
      3
    );
  });
  it("does not retry permanent lookup failures", async () => {
    const getCalls = mockFetch(() => jsonResponse({ error: "denied" }, 401));
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 5,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-dummy")
    );
    expect(resolved).toBeUndefined();
    expect(getCalls().length).toBe(1);
  });
  it("treats a record without response_cache_source_id as the source itself", async () => {
    const getCalls = mockFetch(() =>
      jsonResponse({
        data: {
          response_cache_source_id: null,
          native_tokens_prompt: 10,
          native_tokens_completion: 25,
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
      maxAttempts: 2,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-source")
    );
    expect(resolved).toEqual({
      sourceIds: ["gen-source"],
      usage: SOURCE_USAGE,
    });
    expect(getCalls().length).toBe(1);
  });
  it("warns when the source generation has no usage fields", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      mockFetch(() =>
        jsonResponse({ data: { response_cache_source_id: null } })
      );
      const resolver = makeOpenRouterGenerationResolver({
        apiKey: "test-key",
        baseUrl: "https://example.com",
        pollIntervalMs: 1,
        maxAttempts: 1,
      });
      const resolved = await runPromise(
        resolver.resolveSourceGeneration("gen-source")
      );
      expect(resolved?.usage).toEqual({
        inputTokens: 0,
        outputTokens: 0,
        totalTokens: 0,
        reasoningTokens: 0,
        totalCost: 0,
        generationTimeMs: 0,
      });
      const messages = warn.mock.calls.map((call) => String(call[0]));
      expect(
        messages.some((message) =>
          message.includes("Source generation has no usage fields")
        )
      ).toBe(true);
    } finally {
      warn.mockRestore();
    }
  });
  it("returns undefined after exactly maxAttempts lookups when the row never lands", async () => {
    const getCalls = mockFetch(() => jsonResponse({ error: "not found" }, 404));
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
    expect(resolved).toEqual({ sourceIds: ["gen-original"] });
    expect(getCalls().length).toBe(2);
  });
  it("resolves a server-tools root to its leaf generations", async () => {
    const getCalls = mockFetch((url) => {
      if (url.includes("gen-root")) {
        return jsonResponse({
          data: {
            response_cache_source_id: null,
            related_generation_ids: ["gen-child-1", "gen-child-2"],
          },
        });
      }
      return jsonResponse({
        data: {
          response_cache_source_id: null,
          related_generation_ids: [],
        },
      });
    });
    const resolver = makeOpenRouterGenerationResolver({
      apiKey: "test-key",
      baseUrl: "https://example.com",
      pollIntervalMs: 1,
      maxAttempts: 1,
    });
    const resolved = await runPromise(
      resolver.resolveSourceGeneration("gen-root", {
        includeRelated: true,
        includeUsage: false,
      })
    );
    expect(resolved).toEqual({
      sourceIds: ["gen-child-1", "gen-child-2"],
    });
    expect(getCalls()).toEqual([
      "https://example.com/api/v1/generation?id=gen-root&include_related=true",
      "https://example.com/api/v1/generation?id=gen-child-1&include_related=true",
      "https://example.com/api/v1/generation?id=gen-child-2&include_related=true",
    ]);
  });
});
