import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import assert from "node:assert/strict";

import { FetchHttpClient } from "@effect/platform";
import { either, provide, runPromise } from "effect/Effect";

import { Either } from "../internal/either";
import type { CacheStore } from "./cache-store";
import { fetchCachedTextFile } from "./cached-file";

function makeMemoryStore(overrides?: Partial<CacheStore>): {
  readonly store: CacheStore;
  readonly entries: Map<string, unknown>;
} {
  const entries = new Map<string, unknown>();
  const store: CacheStore = {
    backend: "gcs",
    enabled: true,
    async readJson(key) {
      return entries.get(key);
    },
    async writeJson(key, value) {
      entries.set(key, value);
    },
    async tryHydrateCheckout() {
      return false;
    },
    async snapshotCheckout() {},
    ...overrides,
  };
  return { store, entries };
}

const REQUEST = {
  url: "https://example.test/datasets/owner/name/resolve/main/db.json",
  scope: "hf/owner/name",
  revision: "main",
  filename: "db.json",
} as const;

const CACHE_KEY = "files/hf%2Fowner%2Fname/main/db.json.json";

function run(
  request: Parameters<typeof fetchCachedTextFile>[0]
): Promise<string> {
  return runPromise(
    fetchCachedTextFile(request).pipe(provide(FetchHttpClient.layer))
  );
}

describe("fetchCachedTextFile", () => {
  let originalFetch: typeof global.fetch;
  let requestCount: number;

  beforeEach(() => {
    originalFetch = global.fetch;
    requestCount = 0;
  });

  afterEach(() => {
    global.fetch = originalFetch;
  });

  function stubFetch(responses: readonly Response[]): void {
    global.fetch = (() => {
      const response = responses[Math.min(requestCount, responses.length - 1)];
      requestCount += 1;
      return Promise.resolve(response.clone());
    }) as typeof global.fetch;
  }

  it("stores the downloaded body under the scope/revision key", async () => {
    stubFetch([new Response('{"users":{}}', { status: 200 })]);
    const { store, entries } = makeMemoryStore();

    await expect(run({ ...REQUEST, cacheStore: store })).resolves.toBe(
      '{"users":{}}'
    );
    expect(entries.get(CACHE_KEY)).toEqual({ text: '{"users":{}}' });
    expect(requestCount).toBe(1);
  });

  it("serves a second reader from the cache without another request", async () => {
    stubFetch([new Response('{"users":{}}', { status: 200 })]);
    const { store } = makeMemoryStore();

    await run({ ...REQUEST, cacheStore: store });
    await expect(run({ ...REQUEST, cacheStore: store })).resolves.toBe(
      '{"users":{}}'
    );
    expect(requestCount).toBe(1);
  });

  it("passes maxAgeMs through to the store read", async () => {
    stubFetch([new Response("fresh", { status: 200 })]);
    const reads: (number | undefined)[] = [];
    const { store } = makeMemoryStore({
      async readJson(_key, opts) {
        reads.push(opts?.maxAgeMs);
        return undefined;
      },
    });

    await run({ ...REQUEST, cacheStore: store, maxAgeMs: 1_000 });
    expect(reads).toEqual([1_000]);
  });

  it("refetches when the cached entry is not a text envelope", async () => {
    stubFetch([new Response("body", { status: 200 })]);
    const { store, entries } = makeMemoryStore();
    entries.set(CACHE_KEY, { unexpected: true });

    await expect(run({ ...REQUEST, cacheStore: store })).resolves.toBe("body");
    expect(requestCount).toBe(1);
  });

  it("retries a rate-limited origin and caches the successful body", async () => {
    stubFetch([
      new Response("slow down", { status: 429 }),
      new Response("recovered", { status: 200 }),
    ]);
    const { store, entries } = makeMemoryStore();

    await expect(
      run({
        ...REQUEST,
        cacheStore: store,
        retry: { maxRetries: 2, baseDelayMs: 1 },
      })
    ).resolves.toBe("recovered");
    expect(requestCount).toBe(2);
    expect(entries.get(CACHE_KEY)).toEqual({ text: "recovered" });
  });

  it("fails with the origin status once retries are exhausted", async () => {
    stubFetch([new Response("slow down", { status: 429 })]);
    const { store } = makeMemoryStore();

    const result = await runPromise(
      fetchCachedTextFile({
        ...REQUEST,
        cacheStore: store,
        retry: { maxRetries: 1, baseDelayMs: 1 },
      }).pipe(either, provide(FetchHttpClient.layer))
    );

    assert(Either.isLeft(result));
    assert(result.left._tag === "CachedFileError");
    expect(result.left.status).toBe(429);
  });

  it("treats a 300 response as a failure rather than a body", async () => {
    stubFetch([new Response("moved", { status: 300 })]);
    const { store, entries } = makeMemoryStore();

    const result = await runPromise(
      fetchCachedTextFile({
        ...REQUEST,
        cacheStore: store,
        retry: { maxRetries: 0, baseDelayMs: 1 },
      }).pipe(either, provide(FetchHttpClient.layer))
    );

    assert(Either.isLeft(result));
    assert(result.left._tag === "CachedFileError");
    expect(result.left.status).toBe(300);
    expect(entries.size).toBe(0);
  });

  it("skips the cache entirely when the store is disabled", async () => {
    stubFetch([new Response("body", { status: 200 })]);
    const { store, entries } = makeMemoryStore({ enabled: false });

    await expect(run({ ...REQUEST, cacheStore: store })).resolves.toBe("body");
    expect(entries.size).toBe(0);
  });

  it("returns the body even when the cache write fails", async () => {
    stubFetch([new Response("body", { status: 200 })]);
    const { store } = makeMemoryStore({
      writeJson() {
        return Promise.reject(new Error("gcs down"));
      },
    });

    await expect(run({ ...REQUEST, cacheStore: store })).resolves.toBe("body");
  });
});
