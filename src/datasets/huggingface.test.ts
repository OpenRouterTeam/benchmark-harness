import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";

import { toReadonlyArray } from "effect/Chunk";
import { fromMap } from "effect/ConfigProvider";
import {
  fail,
  flatMap,
  map,
  provide,
  retry,
  succeed,
  suspend,
  withConfigProvider,
} from "effect/Effect";
import { runCollect } from "effect/Stream";

import type { Sample } from "../harness/core";
import { Dataset } from "../harness/dataset";
import { runHarnessPromise } from "../internal/effect-logger";
import type { CacheStore } from "./cache-store";
import {
  HF_CACHED_ASSETS_URL_PREFIX,
  hfFetchRetrySchedule,
  makeHfDatasetLayer,
  resolveHfToken,
} from "./huggingface";

function rowsPage(opts: { numRowsTotal: number; rows: number }): unknown {
  const { numRowsTotal, rows } = opts;
  return {
    rows: Array.from({ length: rows }, (_, i) => ({
      row_idx: i,
      row: { id: i },
    })),
    num_rows_total: numRowsTotal,
  };
}

const headersByRequest: Record<string, string>[] = [];

let restoreFetch: (() => void) | undefined;

function stubFetch(
  response: unknown,
  assetBytes = new Uint8Array(),
  options: {
    readonly assetContentType?: string;
    readonly assetStatuses?: readonly number[];
  } = {}
): void {
  const original = globalThis.fetch;
  let assetRequestCount = 0;
  const stub: typeof fetch = (input, init) => {
    const req =
      input instanceof Request ? input : new Request(String(input), init);
    const headers: Record<string, string> = {};
    req.headers.forEach((value, key) => {
      headers[key] = value;
    });
    headersByRequest.push(headers);
    if (req.url.startsWith(HF_CACHED_ASSETS_URL_PREFIX)) {
      const status =
        options.assetStatuses?.[assetRequestCount] ??
        options.assetStatuses?.at(-1) ??
        200;
      assetRequestCount++;
      const assetHeaders =
        options.assetContentType === undefined
          ? {}
          : { "content-type": options.assetContentType };
      return Promise.resolve(
        new Response(assetBytes, {
          status,
          headers: assetHeaders,
        })
      );
    }
    return Promise.resolve(
      new Response(JSON.stringify(response), {
        status: 200,
        headers: { "content-type": "application/json" },
      })
    );
  };
  globalThis.fetch = stub;
  restoreFetch = () => {
    globalThis.fetch = original;
    restoreFetch = undefined;
  };
}

function recordingCacheStore(writes: unknown[]): CacheStore {
  return {
    backend: "disk",
    enabled: true,
    async readJson() {
      return undefined;
    },
    async writeJson(_key, value) {
      writes.push(value);
    },
    async tryHydrateCheckout() {
      return false;
    },
    async snapshotCheckout() {},
  };
}

function fetchFirstSample(
  layer: ReturnType<typeof makeHfDatasetLayer>
): Promise<Sample | undefined> {
  return runHarnessPromise(
    Dataset.pipe(
      flatMap((d) => runCollect(d.stream({ start: 0, end: 1 }))),
      map((samples) => toReadonlyArray(samples)[0]),
      provide(layer)
    )
  );
}

function fetchOnceWithLayer(
  layer: ReturnType<typeof makeHfDatasetLayer>
): Promise<number> {
  return runHarnessPromise(
    Dataset.pipe(
      flatMap((d) => d.size),
      provide(layer)
    )
  );
}
describe("makeHfDatasetLayer", () => {
  let savedCacheDisable: string | undefined;
  beforeEach(() => {
    savedCacheDisable = process.env.BENCH_DATASET_CACHE_DISABLE;
    process.env.BENCH_DATASET_CACHE_DISABLE = "1";
  });
  afterEach(() => {
    if (savedCacheDisable === undefined) {
      delete process.env.BENCH_DATASET_CACHE_DISABLE;
    } else {
      process.env.BENCH_DATASET_CACHE_DISABLE = savedCacheDisable;
    }
    restoreFetch?.();
    restoreFetch = undefined;
    headersByRequest.length = 0;
  });
  it("sends Authorization: Bearer <hfToken> on /rows requests", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    const layer = makeHfDatasetLayer({
      dataset: "test/dataset",
      inlinePngImages: true,
      config: "default",
      split: "train",
      hfToken: "hf_test_token",
      recordToSample: (record) => ({
        id: String(record["id"] ?? ""),
        input: "unused",
        target: { text: "unused" },
      }),
    });
    const size = await fetchOnceWithLayer(layer);
    expect(size).toBe(1);
    expect(headersByRequest.length).toBe(1);
    expect(headersByRequest[0]?.["authorization"]).toBe("Bearer hf_test_token");
  });
  it("omits Authorization when hfToken is an explicit empty string (anonymous)", async () => {
    stubFetch(rowsPage({ numRowsTotal: 1, rows: 1 }));
    const layer = makeHfDatasetLayer({
      dataset: "test/dataset",
      inlinePngImages: true,
      config: "default",
      split: "train",
      hfToken: "",
      recordToSample: (record) => ({
        id: String(record["id"] ?? ""),
        input: "unused",
        target: { text: "unused" },
      }),
    });
    const size = await fetchOnceWithLayer(layer);
    expect(size).toBe(1);
    expect(headersByRequest.length).toBe(1);
    expect(headersByRequest[0]?.["authorization"]).toBeUndefined();
  });
  it.each([false, true])(
    "only inlines PNG assets when opted in (%s)",
    async (inlinePngImages) => {
      const imageUrl = `${HF_CACHED_ASSETS_URL_PREFIX}x/y.png?Expires=1&Signature=s`;
      const externalImage = {
        src: "https://example.com/a.png",
        height: 10,
        width: 20,
      };
      const row = {
        id: "image-row",
        image: { src: imageUrl, height: 30, width: 40 },
        externalImage,
        description: "preserved",
      };
      let fetchedRecord: Readonly<Record<string, unknown>> | undefined;
      stubFetch(
        {
          rows: [{ row_idx: 0, row }],
          num_rows_total: 1,
        },
        new Uint8Array([0, 1, 2, 255])
      );
      const layer = makeHfDatasetLayer({
        dataset: "test/dataset",
        inlinePngImages,
        config: "default",
        split: "train",
        hfToken: "hf_test_token",
        recordToSample: (record) => {
          fetchedRecord = record;
          return {
            id: String(record["id"] ?? ""),
            input: "unused",
            target: { text: "unused" },
          };
        },
      });
      await fetchFirstSample(layer);
      const image = fetchedRecord?.["image"];
      expect(image).toEqual({
        src: inlinePngImages ? "data:image/png;base64,AAEC/w==" : imageUrl,
        height: 30,
        width: 40,
      });
      expect(fetchedRecord?.["externalImage"]).toEqual(externalImage);
      expect(fetchedRecord?.["description"]).toBe("preserved");
      expect(
        headersByRequest.filter((headers) => headers["authorization"]).length
      ).toBe(inlinePngImages ? 2 : 1);
      expect(
        headersByRequest.some(
          (headers) => headers["authorization"] === "Bearer hf_test_token"
        )
      ).toBe(true);
    }
  );
  it("fails cached asset requests and does not write a failed page", async () => {
    const writes: unknown[] = [];
    stubFetch(
      {
        rows: [
          {
            row_idx: 7,
            row: {
              image: {
                src: `${HF_CACHED_ASSETS_URL_PREFIX}x/y.png?Expires=1&Signature=secret`,
              },
            },
          },
        ],
        num_rows_total: 1,
      },
      new Uint8Array([1, 2, 3]),
      { assetStatuses: [403] }
    );
    const layer = makeHfDatasetLayer({
      dataset: "test/dataset",
      inlinePngImages: true,
      config: "default",
      split: "train",
      hfToken: "",
      retry: { maxRetries: 0, baseDelayMs: 0 },
      cacheStore: recordingCacheStore(writes),
      recordToSample: () => ({
        id: "image",
        input: "unused",
        target: { text: "unused" },
      }),
    });
    const error = await fetchFirstSample(layer).catch(
      (cause: unknown) => cause
    );
    expect(String(error)).toContain("row_idx=7");
    expect(String(error)).not.toContain("Signature");
    expect(writes).toHaveLength(0);
  });
  it("retries a failed cached asset request", async () => {
    let fetchedRecord: Readonly<Record<string, unknown>> | undefined;
    stubFetch(
      {
        rows: [
          {
            row_idx: 0,
            row: {
              image: { src: `${HF_CACHED_ASSETS_URL_PREFIX}x/y.png` },
            },
          },
        ],
        num_rows_total: 1,
      },
      new Uint8Array([0x89, 0x50, 0x4e, 0x47]),
      { assetStatuses: [500, 200], assetContentType: "image/png" }
    );
    const layer = makeHfDatasetLayer({
      dataset: "test/dataset",
      inlinePngImages: true,
      config: "default",
      split: "train",
      hfToken: "",
      retry: { maxRetries: 1, baseDelayMs: 0 },
      recordToSample: (record) => {
        fetchedRecord = record;
        return {
          id: "image",
          input: "unused",
          target: { text: "unused" },
        };
      },
    });
    await fetchFirstSample(layer);
    expect(fetchedRecord?.["image"]).toMatchObject({
      src: "data:image/png;base64,iVBORw==",
    });
    expect(headersByRequest).toHaveLength(3);
  });
});
describe("hfFetchRetrySchedule", () => {
  let restoreWarn: (() => void) | undefined;
  afterEach(() => {
    restoreWarn?.();
    restoreWarn = undefined;
  });
  it("logs each retry attempt", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    restoreWarn = () => warn.mockRestore();
    const error = new Error("HF unavailable");
    let attempts = 0;
    const flaky = suspend(() => {
      attempts++;
      return attempts < 3 ? fail(error) : succeed(attempts);
    });
    const result = await runHarnessPromise(
      flaky.pipe(retry(hfFetchRetrySchedule({ baseDelayMs: 0 })))
    );
    expect(result).toBe(3);
    expect(warn).toHaveBeenCalledTimes(2);
    expect(warn.mock.calls[0]?.[0]).toBe("Retrying after transient error");
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      attempt: 1,
      error_tag: "Error",
      error_message: "HF unavailable",
    });
  });
  it("does not log non-retryable failures", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    restoreWarn = () => warn.mockRestore();
    const error = new Error("not found");
    await expect(
      runHarnessPromise(
        fail(error).pipe(
          retry(hfFetchRetrySchedule({ baseDelayMs: 0 }, () => false))
        )
      )
    ).rejects.toThrow("not found");
    expect(warn).not.toHaveBeenCalled();
  });
});
describe("resolveHfToken", () => {
  it("reads HF_TOKEN from Effect Config", async () => {
    await expect(
      runHarnessPromise(
        withConfigProvider(fromMap(new Map([["HF_TOKEN", "hf_test_token"]])))(
          resolveHfToken()
        )
      )
    ).resolves.toBe("hf_test_token");
  });
  it("defaults to an empty token when HF_TOKEN is unavailable", async () => {
    await expect(
      runHarnessPromise(
        withConfigProvider(fromMap(new Map()))(resolveHfToken())
      )
    ).resolves.toBe("");
  });
});
