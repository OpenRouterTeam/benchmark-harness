import type { Layer } from "effect/Layer";

import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { Sample } from "../../harness/core";
import type { Dataset } from "../../harness/dataset";
import { definedValues } from "../../internal/guards";
import type { RetryConfig } from "../../runtime/retry";
import type { DecisionTaskId, DecisionVariant } from "./schema";
import {
  DECISION_SPEC_METADATA_KEY,
  DECISIONS_BENCHMARK_ID,
  DecisionVariant as Variant,
  DecisionTaskId as TaskId,
} from "./schema";
import { DEFAULT_LANGUAGE, decisionTask } from "./tasks";
import { applyVariant } from "./variants";

export interface DecisionDatasetSelection {
  readonly task: DecisionTaskId;
  readonly variant: DecisionVariant;
  readonly language: string;
}

export const DEFAULT_DATASET_SELECTION: DecisionDatasetSelection = {
  task: TaskId.Banking77,
  variant: Variant.Base,
  language: DEFAULT_LANGUAGE,
};

export function decisionSampleId(
  selection: DecisionDatasetSelection,
  index: number
): string {
  return `${DECISIONS_BENCHMARK_ID}-${selection.task}-${selection.language}-${selection.variant}-${index}`;
}

export function decisionRecordToSample(
  selection: DecisionDatasetSelection,
  record: Readonly<Record<string, unknown>>,
  index: number
): Sample {
  const task = decisionTask(selection.task);
  const spec = applyVariant({
    spec: task.recordToSpec(record),
    index,
    task: task.id,
    questionId: task.questionId,
    variant: selection.variant,
    language: selection.language,
  });
  return {
    id: decisionSampleId(selection, index),
    input: spec.stateText,
    target: { text: spec.gold },
    metadata: { [DECISION_SPEC_METADATA_KEY]: spec },
  };
}

export function makeDecisionDatasetLayer(
  selection: DecisionDatasetSelection,
  retry?: RetryConfig
): Layer<Dataset> {
  const source = decisionTask(selection.task).source(selection.language);
  return makeHfDatasetLayer(
    definedValues({
      dataset: source.dataset,
      config: source.config,
      split: source.split,
      recordToSample: (
        record: Readonly<Record<string, unknown>>,
        index: number
      ): Sample => decisionRecordToSample(selection, record, index),
      retry,
    })
  );
}
