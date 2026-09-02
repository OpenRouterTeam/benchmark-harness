import { FetchHttpClient, HttpClient } from "@effect/platform";
import { fail, flatMap, gen, map, succeed } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";
import type { Stream } from "effect/Stream";
import { unwrap } from "effect/Stream";

import { resolveCacheStore } from "../datasets/cache-store";
import type { HfDatasetConfig } from "../datasets/huggingface";
import { makeHfPageFetcher, paginateHfRows } from "../datasets/huggingface";
import type { Sample } from "../harness/core";
import { DatasetError } from "../harness/core";
import type { DatasetStreamOptions } from "../harness/dataset";
import { Dataset } from "../harness/dataset";
import { Either } from "../internal/either";
import { definedValues } from "../internal/guards";
import type { RetryConfig } from "../runtime/retry";
import type {
  MmluProCotExample,
  MmluProCotExamplesByCategory,
} from "./mmlu-pro-prompt";
import { parseOptions } from "./mmmu-shared";

const MMLU_PRO_DATASET = "TIGER-Lab/MMLU-Pro";

const MMLU_PRO_CONFIG = "default";

const MMLU_PRO_VALIDATION_SPLIT = "validation";

const MMLU_PRO_TEST_SPLIT = "test";

const MMLU_PRO_VALIDATION_ROWS = 100;

const MMLU_PRO_PAGE_SIZE = 100;

const MMLU_PRO_EXAMPLES_PER_CATEGORY = 5;

type MmluProRecordToSample = (
  record: Readonly<Record<string, unknown>>,
  index: number,
  examplesByCategory: MmluProCotExamplesByCategory
) => Sample;

function requiredString(
  record: Readonly<Record<string, unknown>>,
  field: string
): string {
  const value = record[field];
  if (typeof value !== "string") {
    throw new TypeError(
      `mmlu_pro validation record field "${field}" was not a string`
    );
  }
  return value;
}

function toCotExample(
  record: Readonly<Record<string, unknown>>
): MmluProCotExample {
  return {
    question: requiredString(record, "question"),
    options: parseOptions(record["options"]).filter(
      (option) => option !== "N/A"
    ),
    cotContent: requiredString(record, "cot_content"),
  };
}

function collectCotExamples(
  rows: readonly {
    readonly row: Readonly<Record<string, unknown>>;
  }[]
): MmluProCotExamplesByCategory {
  const examples = new Map<string, MmluProCotExample[]>();
  for (const { row } of rows) {
    const category = requiredString(row, "category");
    const current = examples.get(category);
    if (current === undefined) {
      examples.set(category, [toCotExample(row)]);
    } else if (current.length < MMLU_PRO_EXAMPLES_PER_CATEGORY) {
      current.push(toCotExample(row));
    }
  }
  return examples;
}

function makeDatasetConfig(
  split: string,
  retryConfig?: RetryConfig
): HfDatasetConfig {
  return {
    dataset: MMLU_PRO_DATASET,
    config: MMLU_PRO_CONFIG,
    split,
    pageSize: MMLU_PRO_PAGE_SIZE,
    recordToSample: () => {
      throw new Error(
        "recordToSample is not used by the shared MMLU-Pro dataset layer"
      );
    },
    ...definedValues({
      retry: retryConfig,
    }),
  };
}

export function makeMmluProFewShotDatasetLayer(
  recordToSample: MmluProRecordToSample,
  retryConfig?: RetryConfig,
  hfToken?: string
): Layer<Dataset> {
  const validationConfig = {
    ...makeDatasetConfig(MMLU_PRO_VALIDATION_SPLIT, retryConfig),
    ...definedValues({
      hfToken,
    }),
  };
  const testConfig = {
    ...makeDatasetConfig(MMLU_PRO_TEST_SPLIT, retryConfig),
    ...definedValues({
      hfToken,
    }),
  };
  const makeService = gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const store = resolveCacheStore();
    const fetchValidationPage = makeHfPageFetcher(
      validationConfig,
      client,
      store
    );
    const fetchTestPage = makeHfPageFetcher(testConfig, client, store);
    const size = fetchTestPage(0, 1).pipe(map((page) => page.num_rows_total));
    const stream = (
      opts?: DatasetStreamOptions
    ): Stream<Sample, DatasetError> =>
      unwrap(
        fetchValidationPage(0, MMLU_PRO_VALIDATION_ROWS).pipe(
          flatMap((validationPage) => {
            const examplesResult = Either.try(() =>
              collectCotExamples(validationPage.rows)
            );
            if (Either.isLeft(examplesResult)) {
              return fail(
                new DatasetError({
                  message: `Failed to map MMLU-Pro validation records: ${String(examplesResult.left)}`,
                })
              );
            }
            return succeed(examplesResult.right);
          }),
          map((examplesByCategory) =>
            paginateHfRows({
              fetchPage: fetchTestPage,
              pageSize: MMLU_PRO_PAGE_SIZE,
              dataset: MMLU_PRO_DATASET,
              start: opts?.start,
              end: opts?.end,
              mapRow: (row, index) =>
                recordToSample(row.row, index, examplesByCategory),
            })
          )
        )
      );
    return Dataset.of({ stream, size });
  });
  return effect(Dataset, makeService).pipe(provide(FetchHttpClient.layer));
}
