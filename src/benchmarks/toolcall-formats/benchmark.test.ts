import { describe, expect, it } from "bun:test";

import { toArray } from "effect/Chunk";
import { gen, provide, runPromise } from "effect/Effect";
import { runCollect } from "effect/Stream";

import { Dataset } from "../../harness/dataset";
import { TOOLCALL_FORMATS_META } from "../benchmark-meta";
import { getBenchmark } from "../registry";
import {
  makeToolCallFormatsDatasetLayer,
  TOOLCALL_FORMATS_BENCHMARK,
  TOOLCALL_FORMATS_SAMPLES,
} from "./benchmark";
import { TOOLCALL_FORMAT_CASES } from "./cases";

describe("toolcall_formats benchmark", () => {
  it("is registered with its metadata", () => {
    expect(getBenchmark(TOOLCALL_FORMATS_META.id)).toBe(
      TOOLCALL_FORMATS_BENCHMARK
    );
    expect(TOOLCALL_FORMATS_BENCHMARK.defaultEpochs).toBe(
      TOOLCALL_FORMATS_META.defaultEpochs
    );
  });

  it("streams one sample per case and honors start/end", async () => {
    const program = gen(function* () {
      const dataset = yield* Dataset;
      const size = yield* dataset.size;
      const slice = yield* runCollect(dataset.stream({ start: 2, end: 5 }));
      return { size, ids: toArray(slice).map((sample) => sample.id) };
    });
    const result = await runPromise(
      program.pipe(provide(makeToolCallFormatsDatasetLayer()))
    );
    expect(result.size).toBe(TOOLCALL_FORMAT_CASES.length);
    expect(result.ids).toEqual(
      TOOLCALL_FORMATS_SAMPLES.slice(2, 5).map((sample) => sample.id)
    );
  });
});
