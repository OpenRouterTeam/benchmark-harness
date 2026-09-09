import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import assert from "node:assert/strict";

import { FetchHttpClient } from "@effect/platform";
import { either, provide, runPromise } from "effect/Effect";

import { Either } from "../internal/either";
import type { CacheStore } from "./cache-store";
import {
  fetchCachedTextFile,
  isHuggingFaceUrl,
  parseRetryAfterMs,
} from "./cached-file";

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
  url: "https://example.test/datasets/owner/name/resolve/abc123/db.json",
} as const;

const HF_REQUEST = {
  url: "https://huggingface.co/datasets/owner/name/resolve/abc123/db.json",
} as const;

const CACHE_KEY = `files/${encodeURIComponent(REQUEST.url)}.json`;

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
  let sentHeaders: Headers[];

  beforeEach(() => {
    originalFetch = global.fetch;
    requestCount = 0;
    sentHeaders = [];
  });

  afterEach(() => {
    global.fetch = originalFetch;
  });

  function stubFetch(responses: readonly Response[]): void {
    const stub: typeof global.fetch = (_input, init) => {
      const response = responses[Math.min(requestCount, responses.length - 1)];
      requestCount += 1;
      sentHeaders.push(new Headers(init?.headers));
      return Promise.resolve(response.clone());
    };
    global.fetch = stub;
  }

  it("stores the downloaded body under the url key", async () => {
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

  it("does not retry a non-retryable status", async () => {
    stubFetch([
      new Response("missing", { status: 404 }),
      new Response("never", { status: 200 }),
    ]);
    const { store, entries } = makeMemoryStore();

    const result = await runPromise(
      fetchCachedTextFile({
        ...REQUEST,
        cacheStore: store,
        retry: { maxRetries: 3, baseDelayMs: 1 },
      }).pipe(either, provide(FetchHttpClient.layer))
    );

    assert(Either.isLeft(result));
    assert(result.left._tag === "CachedFileError");
    expect(result.left.status).toBe(404);
    expect(requestCount).toBe(1);
    expect(entries.size).toBe(0);
  });

  it("retries a 5xx response", async () => {
    stubFetch([
      new Response("upstream", { status: 503 }),
      new Response("recovered", { status: 200 }),
    ]);
    const { store } = makeMemoryStore();

    await expect(
      run({
        ...REQUEST,
        cacheStore: store,
        retry: { maxRetries: 1, baseDelayMs: 1 },
      })
    ).resolves.toBe("recovered");
    expect(requestCount).toBe(2);
  });

  it("waits for retry-after before retrying a 429", async () => {
    stubFetch([
      new Response("slow down", {
        status: 429,
        headers: { "retry-after": "1" },
      }),
      new Response("recovered", { status: 200 }),
    ]);
    const { store } = makeMemoryStore();
    const startedAt = performance.now();

    await expect(
      run({
        ...REQUEST,
        cacheStore: store,
        retry: { maxRetries: 1, baseDelayMs: 1 },
      })
    ).resolves.toBe("recovered");
    expect(performance.now() - startedAt).toBeGreaterThanOrEqual(900);
    expect(requestCount).toBe(2);
  });

  it("sends the HF token only to huggingface.co", async () => {
    stubFetch([new Response("body", { status: 200 })]);
    const { store } = makeMemoryStore();

    await run({ ...HF_REQUEST, cacheStore: store, hfToken: "hf_test" });
    await run({ ...REQUEST, cacheStore: store, hfToken: "hf_test" });

    expect(sentHeaders.map((h) => h.get("authorization"))).toEqual([
      "Bearer hf_test",
      null,
    ]);
  });

  it("sends no authorization header when the token is empty", async () => {
    stubFetch([new Response("body", { status: 200 })]);
    const { store } = makeMemoryStore();

    await run({ ...HF_REQUEST, cacheStore: store, hfToken: "" });

    expect(sentHeaders[0]?.get("authorization")).toBeNull();
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

describe("isHuggingFaceUrl", () => {
  it("matches huggingface.co and its subdomains only", () => {
    expect(isHuggingFaceUrl(HF_REQUEST.url)).toBe(true);
    expect(isHuggingFaceUrl("https://cdn-lfs.huggingface.co/x")).toBe(true);
    expect(isHuggingFaceUrl("https://nothuggingface.co/x")).toBe(false);
    expect(isHuggingFaceUrl(REQUEST.url)).toBe(false);
    expect(isHuggingFaceUrl("not a url")).toBe(false);
  });
});

describe("parseRetryAfterMs", () => {
  it("parses delay seconds and http dates", () => {
    const now = Date.UTC(2026, 0, 1, 0, 0, 0);
    expect(parseRetryAfterMs(undefined, now)).toBeUndefined();
    expect(parseRetryAfterMs("2", now)).toBe(2000);
    expect(parseRetryAfterMs("-1", now)).toBeUndefined();
    expect(parseRetryAfterMs("Thu, 01 Jan 2026 00:00:05 GMT", now)).toBe(5000);
    expect(parseRetryAfterMs("Wed, 31 Dec 2025 23:59:00 GMT", now)).toBe(0);
    expect(parseRetryAfterMs("soon", now)).toBeUndefined();
  });
});
