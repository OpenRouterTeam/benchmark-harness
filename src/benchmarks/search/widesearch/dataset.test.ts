import { afterEach, describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";

import {
  fromUnknown,
  layer as configProviderLayer,
} from "effect/ConfigProvider";
import type { Effect } from "effect/Effect";
import { flatMap, provide, runPromise } from "effect/Effect";

import { Dataset } from "../../../harness/dataset";
import { assertFailure, assertSuccess } from "../../../internal/testing";
import {
  makeWideSearchDatasetLayer,
  parseWideSearchGoldCsv,
  parseWideSearchSource,
} from "./dataset";

const SOURCE_ROW = {
  instance_id: "ws_en_001",
  query: "Build a table.",
  evaluation: JSON.stringify({
    required: ["Name"],
    unique_columns: ["Name"],
    eval_pipeline: { Name: {} },
  }),
  language: "en",
};

const headersByRequest: Record<string, string>[] = [];

let restoreFetch: (() => void) | undefined;

function stubFetch(): void {
  const original = globalThis.fetch;
  globalThis.fetch = (input, init) => {
    const request =
      input instanceof Request ? input : new Request(String(input), init);
    const headers: Record<string, string> = {};
    request.headers.forEach((value, key) => {
      headers[key] = value;
    });
    headersByRequest.push(headers);
    return Promise.resolve(new Response("not the pinned WideSearch dataset"));
  };
  restoreFetch = () => {
    globalThis.fetch = original;
    restoreFetch = undefined;
  };
}

function fetchDatasetSize(): Effect<number, unknown, never> {
  return Dataset.pipe(
    flatMap((dataset) => dataset.size),
    provide(makeWideSearchDatasetLayer({ maxRetries: 0, baseDelayMs: 0 }))
  );
}
describe("WideSearch dataset", () => {
  afterEach(() => {
    restoreFetch?.();
    restoreFetch = undefined;
    headersByRequest.length = 0;
  });
  it("validates the pinned source checksum and row count", () => {
    const text = `${JSON.stringify(SOURCE_ROW)}\n${JSON.stringify(SOURCE_ROW)}\n`;
    const sha256 = createHash("sha256").update(text, "utf8").digest("hex");
    const result = parseWideSearchSource(text, sha256, 2);
    assertSuccess(result);
    expect(result.success).toHaveLength(2);
  });
  it("fails closed on source checksum drift", () => {
    const result = parseWideSearchSource(
      `${JSON.stringify(SOURCE_ROW)}\n`,
      "wrong",
      1
    );
    assertFailure(result);
    expect(result.failure).toContain("WideSearch checksum mismatch");
  });
  it("loads normalized required columns from official gold CSVs", () => {
    const result = parseWideSearchGoldCsv(
      "\uFEFFName,Pack Size,Unused\nA,750ml,x\n",
      "ws_1",
      ["name", "packsize"]
    );
    assertSuccess(result);
    expect(result.success).toEqual([{ name: "A", packsize: "750ml" }]);
  });
  it("rejects empty or incomplete gold CSVs", () => {
    assertFailure(parseWideSearchGoldCsv("Name\n", "ws_1", ["name"]));
    assertFailure(parseWideSearchGoldCsv("Other\nx\n", "ws_1", ["name"]));
  });
  it("sends the configured HF authorization header from the dataset layer", async () => {
    stubFetch();
    await expect(
      runPromise(
        provide(
          fetchDatasetSize(),
          configProviderLayer(fromUnknown({ HF_TOKEN: "hf_test_token" }))
        )
      )
    ).rejects.toThrow("checksum mismatch");
    expect(headersByRequest[0]?.["authorization"]).toBe("Bearer hf_test_token");
  });
  it("omits the HF authorization header when no token is configured", async () => {
    stubFetch();
    await expect(
      runPromise(
        provide(fetchDatasetSize(), configProviderLayer(fromUnknown({})))
      )
    ).rejects.toThrow("checksum mismatch");
    expect(headersByRequest[0]?.["authorization"]).toBeUndefined();
  });
});
