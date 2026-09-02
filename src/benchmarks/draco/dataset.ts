import type { Layer } from "effect/Layer";

import type { HfDatasetConfig } from "../../datasets/huggingface";
import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { Sample } from "../../harness/core";
import type { Dataset } from "../../harness/dataset";
import { Either } from "../../internal/either";
import { definedValues, isRecord } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import type { RetryConfig } from "../../runtime/retry";
import { extractCriteria } from "./criteria";
import { CriterionSchema, DracoTaskSchema } from "./schemas";

export const DRACO_DATASET_REPO = "perplexity-ai/draco";

export const DRACO_DATASET_CONFIG = "default";

export const DRACO_DATASET_SPLIT = "test";

function parseAnswer(raw: unknown): Record<string, unknown> {
  if (typeof raw === "string") {
    const parsed = Either.try(() => JSON.parse(raw));
    if (Either.isRight(parsed) && isRecord(parsed.right)) {
      return parsed.right;
    }
  }
  if (isRecord(raw)) {
    return raw;
  }
  return {};
}

export function dracoRecordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number
): Sample {
  const taskFields = parseSchema(
    DracoTaskSchema.pick({ problem: true, domain: true }),
    record
  );
  if (Either.isLeft(taskFields)) {
    throw new Error(
      `DRACO row ${index} failed validation: ${taskFields.left.message}`
    );
  }
  const { problem, domain } = taskFields.right;
  const id = typeof record["id"] === "string" ? record["id"] : `draco-${index}`;
  const answer = parseAnswer(record["answer"]);
  const criteria = extractCriteria(answer);
  const validatedCriteria = parseSchema(CriterionSchema.array(), criteria);
  if (Either.isLeft(validatedCriteria)) {
    throw new Error(
      `DRACO row ${index} (id=${id}) produced invalid criteria: ${validatedCriteria.left.message}`
    );
  }
  return {
    id,
    input: problem,
    target: { text: "" },
    metadata: {
      domain,
      index,
      criteria: validatedCriteria.right,
      rawAnswer: answer,
    },
  };
}

export const DRACO_DATASET = {
  dataset: DRACO_DATASET_REPO,
  config: DRACO_DATASET_CONFIG,
  split: DRACO_DATASET_SPLIT,
  recordToSample: dracoRecordToSample,
} as const satisfies Omit<HfDatasetConfig, "pageSize">;

export function makeDracoDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  return makeHfDatasetLayer({
    ...DRACO_DATASET,
    ...definedValues({
      retry: retryConfig,
    }),
  });
}
