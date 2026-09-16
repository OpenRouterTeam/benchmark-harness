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
import {
  HF_CACHED_ASSETS_URL_PREFIX,
  hfFetchRetrySchedule,
  isHfCachedAssetUrl,
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

function stubFetch(response: unknown, assetBytes = new Uint8Array()): void {
  const original = globalThis.fetch;
  const stub: typeof fetch = (input, init) => {
    const req =
      input instanceof Request ? input : new Request(String(input), init);
    const headers: Record<string, string> = {};
    req.headers.forEach((value, key) => {
      headers[key] = value;
    });
    headersByRequest.push(headers);
    if (req.url.startsWith(HF_CACHED_ASSETS_URL_PREFIX)) {
      return Promise.resolve(
        new Response(assetBytes, {
          status: 200,
          headers: { "content-type": "image/png; charset=utf-8" },
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
  it("inlines cached asset images while preserving other row values", async () => {
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
      src: "data:image/png;base64,AAEC/w==",
      height: 30,
      width: 40,
    });
    expect(fetchedRecord?.["externalImage"]).toEqual(externalImage);
    expect(fetchedRecord?.["description"]).toBe("preserved");
    expect(
      headersByRequest.filter((headers) => headers["authorization"]).length
    ).toBe(2);
    expect(
      headersByRequest.some(
        (headers) => headers["authorization"] === "Bearer hf_test_token"
      )
    ).toBe(true);
  });
});
describe("isHfCachedAssetUrl", () => {
  it("recognizes only the Hugging Face cached-assets prefix", () => {
    expect(
      isHfCachedAssetUrl(`${HF_CACHED_ASSETS_URL_PREFIX}x/y.png?Expires=1`)
    ).toBe(true);
    expect(isHfCachedAssetUrl("https://example.com/a.png")).toBe(false);
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
