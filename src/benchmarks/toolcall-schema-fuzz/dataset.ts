import { fromIterable } from "effect/Chunk";
import { succeed as effectSucceed } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";
import { fromChunk } from "effect/Stream";

import type { Sample } from "../../harness/core";
import { Dataset } from "../../harness/dataset";
import type { ToolCallSchemaFuzzCase } from "./tool-case";

function fuzzCaseToSample(entry: ToolCallSchemaFuzzCase): Sample {
  return {
    id: entry.id,
    input: entry.prompt,
    target: { text: JSON.stringify(entry.calls) },
    metadata: {
      scenario: entry.scenario,
      tools: entry.tools.map((tool) => tool.function.name),
      strict: entry.strict,
      constructs: [...entry.constructs],
      ...(entry.control === undefined ? {} : { control: entry.control }),
    },
  };
}

export function makeToolCallSchemaFuzzDatasetLayer(
  cases: readonly ToolCallSchemaFuzzCase[]
): Layer<Dataset> {
  const samples = cases.map(fuzzCaseToSample);
  return layerSucceed(
    Dataset,
    Dataset.of({
      stream: (opts) =>
        fromChunk(fromIterable(samples.slice(opts?.start, opts?.end))),
      size: effectSucceed(samples.length),
    })
  );
}
