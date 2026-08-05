import { afterEach, describe, expect, it } from "bun:test";

import { toReadonlyArray } from "effect/Chunk";
import { gen, provide, runPromise } from "effect/Effect";
import { runCollect } from "effect/Stream";

import { Dataset } from "../../../harness/dataset";
import { assertLeft, assertRight } from "../../../internal/testing";
import {
  HLE_ACCESS_URL,
  HLE_REVISION,
  HLE_TEXT_ONLY_ROWS,
  HLE_TEXT_ONLY_WHERE,
  hleRecordToSample,
  makeHleDatasetLayer,
} from "./dataset";

const RECORD = {
  id: "upstream-id",
  question: "What is the answer?",
  image: "",
  answer: "42",
  answer_type: "exactMatch",
  raw_subject: "Mathematics",
  category: "Math",
};

const requestUrls: string[] = [];

let restoreFetch: (() => void) | undefined;

function stubRows(source: readonly Readonly<Record<string, unknown>>[]): void {
  stubFetch((request) => {
    if (request.url.includes("/api/datasets/")) {
      return { sha: HLE_REVISION };
    }
    const url = new URL(request.url);
    const offset = Number(url.searchParams.get("offset"));
    const length = Number(url.searchParams.get("length"));
    const filtered = source
      .map((row, row_idx) => ({ row_idx, row }))
      .filter(({ row }) => row["image"] === "");
    return {
      rows: filtered.slice(offset, offset + length),
      num_rows_total: filtered.length,
    };
  });
}

function stubFetch(
  bodyForRequest: (request: Request) => unknown,
  status = 200
): void {
  const original = globalThis.fetch;
  globalThis.fetch = (input, init) => {
    const request =
      input instanceof Request ? input : new Request(String(input), init);
    requestUrls.push(request.url);
    return Promise.resolve(
      new Response(JSON.stringify(bodyForRequest(request)), {
        status,
        headers: { "content-type": "application/json" },
      })
    );
  };
  restoreFetch = () => {
    globalThis.fetch = original;
    restoreFetch = undefined;
  };
}
describe("HLE dataset", () => {
  afterEach(() => {
    restoreFetch?.();
    requestUrls.length = 0;
  });
  it("preserves answer metadata from official rows", () => {
    const result = hleRecordToSample(RECORD);
    assertRight(result);
    expect(result.right).toEqual({
      id: "upstream-id",
      input: "What is the answer?",
      target: { text: "42" },
      metadata: {
        answer_type: "exactMatch",
        raw_subject: "Mathematics",
        category: "Math",
      },
    });
  });
  it("rejects malformed official rows", () => {
    const result = hleRecordToSample({ ...RECORD, category: null });
    assertLeft(result);
    expect(result.left).toContain("invalid HLE record");
  });
  it("filters image questions before applying ranges", async () => {
    const source = Array.from(
      { length: HLE_TEXT_ONLY_ROWS + 1 },
      (_, index) => ({
        ...RECORD,
        id: `hle-${index}`,
        image: index === 1 ? "image.png" : "",
      })
    );
    stubRows(source);
    const result = await runPromise(
      gen(function* () {
        const dataset = yield* Dataset;
        const size = yield* dataset.size;
        const samples = yield* runCollect(dataset.stream({ start: 0, end: 2 }));
        return { size, samples: toReadonlyArray(samples) };
      }).pipe(provide(makeHleDatasetLayer({ baseDelayMs: 0 }, "hf_test")))
    );
    expect(result.size).toBe(HLE_TEXT_ONLY_ROWS);
    expect(result.samples.map((sample) => sample.id)).toEqual([
      "hle-0",
      "hle-2",
    ]);
    expect(new URL(requestUrls[1]!).searchParams.get("where")).toBe(
      HLE_TEXT_ONLY_WHERE
    );
  });
  it("fails with gated-access guidance", async () => {
    stubFetch(() => ({ error: "Unauthorized" }), 401);
    const program = gen(function* () {
      const dataset = yield* Dataset;
      return yield* dataset.size;
    }).pipe(provide(makeHleDatasetLayer({ maxRetries: 0 }, "unauthorized")));
    await expect(runPromise(program)).rejects.toThrow(HLE_ACCESS_URL);
  });
});
