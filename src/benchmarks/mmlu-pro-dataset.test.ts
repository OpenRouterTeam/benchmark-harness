import { afterEach, describe, expect, it } from "bun:test";

import { toReadonlyArray } from "effect/Chunk";
import { flatMap, provide, runPromise } from "effect/Effect";
import { runCollect } from "effect/Stream";

import { Dataset } from "../harness/dataset";
import { mmluProRecordToSample } from "./mmlu-pro";
import { makeMmluProFewShotDatasetLayer } from "./mmlu-pro-dataset";

const validationRows = Array.from({ length: 5 }, (_, index) => ({
  row_idx: index,
  row: {
    question: `Example ${index}`,
    options: ["A example", "B example"],
    cot_content: `A: Example reasoning ${index}. The answer is (A).`,
    category: "business",
    src: "validation",
  },
}));

const testRows = [
  {
    row_idx: 0,
    row: {
      question: "Test question",
      options: ["first", "second"],
      answer: "B",
      category: "business",
      src: "test",
    },
  },
];

let restoreFetch: (() => void) | undefined;

function stubFetch(): void {
  const original = globalThis.fetch;
  globalThis.fetch = (input, init) => {
    const request =
      input instanceof Request ? input : new Request(String(input), init);
    const url = new URL(request.url);
    const rows =
      url.searchParams.get("split") === "validation"
        ? validationRows
        : testRows;
    const response = {
      rows,
      num_rows_total: rows.length,
    };
    return Promise.resolve(
      new Response(JSON.stringify(response), {
        status: 200,
        headers: { "content-type": "application/json" },
      })
    );
  };
  restoreFetch = () => {
    globalThis.fetch = original;
    restoreFetch = undefined;
  };
}
describe("makeMmluProFewShotDatasetLayer", () => {
  afterEach(() => {
    restoreFetch?.();
    restoreFetch = undefined;
  });
  it("loads validation exemplars once and assembles them into test prompts", async () => {
    stubFetch();
    const layer = makeMmluProFewShotDatasetLayer(
      mmluProRecordToSample,
      { baseDelayMs: 0 },
      ""
    );
    const samples = await runPromise(
      Dataset.pipe(
        flatMap((dataset) => runCollect(dataset.stream())),
        provide(layer)
      )
    );
    const [sample] = toReadonlyArray(samples);
    expect(sample?.input).toContain("Question: Example 0");
    expect(sample?.input).toContain("Question: Example 4");
    expect(sample?.input).toContain("Question: Test question");
  });
});
