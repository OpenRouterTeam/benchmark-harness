import { describe, expect, it } from "bun:test";
import assert from "node:assert";

import { toReadonlyArray } from "effect/Chunk";
import { gen, provide, runPromise } from "effect/Effect";
import { runCollect } from "effect/Stream";

import { Dataset } from "../../harness/dataset";
import { toolCallSchemaFuzzCases } from "./cases";
import { makeToolCallSchemaFuzzDatasetLayer } from "./dataset";
import { ToolCallSchemaFuzzGenerator } from "./options";

const REALISTIC = toolCallSchemaFuzzCases({
  generator: ToolCallSchemaFuzzGenerator.Realistic,
});

describe("makeToolCallSchemaFuzzDatasetLayer", () => {
  it("serves every realistic case and honours range slicing", async () => {
    const { size, slice } = await runPromise(
      gen(function* () {
        const dataset = yield* Dataset;
        const total = yield* dataset.size;
        const chunk = yield* runCollect(dataset.stream({ start: 5, end: 10 }));
        return { size: total, slice: toReadonlyArray(chunk) };
      }).pipe(provide(makeToolCallSchemaFuzzDatasetLayer(REALISTIC)))
    );

    expect(size).toBe(REALISTIC.length);
    expect(slice.map((sample) => sample.id)).toEqual(
      REALISTIC.slice(5, 10).map((entry) => entry.id)
    );
    const entry = REALISTIC[5];
    assert(entry !== undefined);
    expect(slice[0]).toStrictEqual({
      id: entry.id,
      input: entry.prompt,
      target: { text: JSON.stringify(entry.calls) },
      metadata: {
        scenario: entry.scenario,
        tools: entry.tools.map((tool) => tool.function.name),
        strict: entry.strict,
        constructs: entry.constructs,
        ...(entry.control === undefined ? {} : { control: entry.control }),
      },
    });
  });
});
