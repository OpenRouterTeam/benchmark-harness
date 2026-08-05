import { succeed } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";
import type { Stream } from "effect/Stream";
import { fromIterable } from "effect/Stream";

import { makeHfDatasetLayer } from "../../datasets/huggingface";
/**
 * Dataset layers for declarative custom evals: inline cases (materialized in
 * the spec) or a HuggingFace dataset with declared input/target fields.
 */
import type { Sample } from "../../harness/core";
import type { Dataset, DatasetStreamOptions } from "../../harness/dataset";
import { Dataset as DatasetTag } from "../../harness/dataset";
import type { RetryConfig } from "../../runtime/retry";
import type { EvalDataset, InlineCase } from "./spec";

export function inlineCaseToSample(
  evalCase: InlineCase,
  index: number
): Sample {
  return {
    id: evalCase.id ?? `custom_eval-${index}`,
    input: evalCase.input,
    target: { text: evalCase.target },
    ...(evalCase.metadata !== undefined && { metadata: evalCase.metadata }),
  };
}

function makeInlineDatasetLayer(cases: readonly InlineCase[]): Layer<Dataset> {
  const samples = cases.map(inlineCaseToSample);
  const stream = (opts?: DatasetStreamOptions): Stream<Sample, never> =>
    fromIterable(samples.slice(opts?.start ?? 0, opts?.end ?? samples.length));
  return layerSucceed(
    DatasetTag,
    DatasetTag.of({ stream, size: succeed(samples.length) })
  );
}

function asString(value: unknown, field: string): string {
  if (typeof value === "string") {
    return value;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  throw new TypeError(
    `custom_eval record field "${field}" is not a string/number/boolean`
  );
}

export function makeCustomEvalDatasetLayer(
  dataset: EvalDataset,
  retryConfig?: RetryConfig
): Layer<Dataset> {
  if (dataset.kind === "inline") {
    return makeInlineDatasetLayer(dataset.cases);
  }
  return makeHfDatasetLayer({
    dataset: dataset.dataset,
    config: dataset.config,
    split: dataset.split,
    ...(dataset.revision !== undefined && { revision: dataset.revision }),
    recordToSample: (record, index) => ({
      id: `custom_eval-${index}`,
      input: asString(record[dataset.inputField], dataset.inputField),
      target: {
        text: asString(record[dataset.targetField], dataset.targetField),
      },
    }),
    ...(retryConfig !== undefined && { retry: retryConfig }),
  });
}
