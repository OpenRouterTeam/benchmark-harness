import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, rmSync, utimesSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { flatMap, provide } from "effect/Effect";

import { Dataset } from "../harness/dataset";
import { runHarnessPromise } from "../internal/effect-logger";
import { makeHfDatasetLayer } from "./huggingface";
import { encodeCacheKeySegment } from "./local-cache";

const ENV_VARS: readonly string[] = [
  "BENCH_DATASET_CACHE_DIR",
  "BENCH_DATASET_CACHE_DISABLE",
  "BENCH_HF_CACHE_TTL_MS",
];

function rowsPage(opts: { numRowsTotal: number; rows: number }): unknown {
  return {
    rows: Array.from({ length: opts.rows }, (_, i) => ({
      row_idx: i,
      row: { id: i },
    })),
    num_rows_total: opts.numRowsTotal,
  };
}

let fetchCount = 0;
let restoreFetch: (() => void) | undefined;

function stubFetch(response: unknown): void {
  const original = globalThis.fetch;
  fetchCount = 0;
  globalThis.fetch = async () => {
    fetchCount += 1;
    return new Response(JSON.stringify(response), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  restoreFetch = () => {
    globalThis.fetch = original;
    restoreFetch = undefined;
  };
}

function makeLayer(opts: { revision?: string }) {
  return makeHfDatasetLayer({
    dataset: "test/dataset",
    config: "default",
    split: "train",
    hfToken: "",
    ...(opts.revision !== undefined && { revision: opts.revision }),
    recordToSample: (record) => ({
      id: String(record["id"] ?? ""),
      input: "unused",
      target: { text: "unused" },
    }),
  });
}

function fetchSize(
  layer: ReturnType<typeof makeHfDatasetLayer>
): Promise<number> {
  return runHarnessPromise(
    Dataset.pipe(
      flatMap((d) => d.size),
      provide(layer)
    )
  );
}

describe("huggingface page cache", () => {
  const saved = Object.fromEntries(ENV_VARS.map((n) => [n, process.env[n]]));
  const tmpDirs: string[] = [];

  beforeEach(() => {
    const dir = mkdtempSync(join(tmpdir(), "hf-cache-test-"));
    tmpDirs.push(dir);
    process.env.BENCH_DATASET_CACHE_DIR = dir;
    delete process.env.BENCH_DATASET_CACHE_DISABLE;
    delete process.env.BENCH_HF_CACHE_TTL_MS;
  });

  afterEach(() => {
    for (const name of ENV_VARS) {
      const value = saved[name];
      if (value === undefined) {
        delete process.env[name];
      } else {
        process.env[name] = value;
      }
    }
    restoreFetch?.();
    restoreFetch = undefined;
    while (tmpDirs.length > 0) {
      const dir = tmpDirs.pop();
      if (dir !== undefined) {
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });

  function cacheFile(opts: { revision?: string; token?: string }): string {
    const root = process.env.BENCH_DATASET_CACHE_DIR;
    if (root === undefined) {
      throw new Error("BENCH_DATASET_CACHE_DIR not set");
    }
    const token = opts.token ?? "";
    const tokenSegment =
      token === ""
        ? "anon"
        : createHash("sha256").update(token).digest("hex").slice(0, 16);
    return join(
      root,
      "hf",
      tokenSegment,
      encodeCacheKeySegment("test/dataset"),
      "default",
      "train",
      encodeCacheKeySegment(opts.revision ?? "HEAD"),
      "0-1.json"
    );
  }

  it("serves repeat requests from disk without refetching", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(fetchCount).toBe(1);
  });

  it("refreshes unpinned (HEAD) entries after the TTL expires", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(fetchCount).toBe(1);
    const file = cacheFile({});
    const old = new Date(Date.now() - 48 * 60 * 60 * 1e3);
    utimesSync(file, old, old);
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(fetchCount).toBe(2);
  });

  it("never expires entries pinned to an explicit revision", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    expect(await fetchSize(makeLayer({ revision: "abc123" }))).toBe(1);
    expect(fetchCount).toBe(1);
    const file = cacheFile({ revision: "abc123" });
    const ancient = new Date(Date.now() - 365 * 24 * 60 * 60 * 1e3);
    utimesSync(file, ancient, ancient);
    expect(await fetchSize(makeLayer({ revision: "abc123" }))).toBe(1);
    expect(fetchCount).toBe(1);
  });

  it("expires revision-pinned entries too when BENCH_HF_CACHE_TTL_MS is set explicitly", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    process.env.BENCH_HF_CACHE_TTL_MS = "0";
    expect(await fetchSize(makeLayer({ revision: "abc123" }))).toBe(1);
    expect(await fetchSize(makeLayer({ revision: "abc123" }))).toBe(1);
    expect(fetchCount).toBe(2);
  });

  it("honors BENCH_HF_CACHE_TTL_MS for unpinned entries", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    process.env.BENCH_HF_CACHE_TTL_MS = "0";
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(fetchCount).toBe(2);
  });

  it("bypasses the cache entirely when BENCH_DATASET_CACHE_DISABLE=1", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    process.env.BENCH_DATASET_CACHE_DISABLE = "1";
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(fetchCount).toBe(2);
  });

  it("scopes cache entries by HF token so anonymous callers never read gated pages", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    const tokenLayer = makeHfDatasetLayer({
      dataset: "test/dataset",
      config: "default",
      split: "train",
      hfToken: "hf_secret_token",
      recordToSample: (record) => ({
        id: String(record["id"] ?? ""),
        input: "unused",
        target: { text: "unused" },
      }),
    });
    expect(await fetchSize(tokenLayer)).toBe(1);
    expect(await fetchSize(tokenLayer)).toBe(1);
    expect(fetchCount).toBe(1);
    expect(existsSync(cacheFile({ token: "hf_secret_token" }))).toBe(true);
    expect(await fetchSize(makeLayer({}))).toBe(1);
    expect(fetchCount).toBe(2);
    expect(existsSync(cacheFile({}))).toBe(true);
  });
});
