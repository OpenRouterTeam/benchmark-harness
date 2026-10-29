import { createHash } from "node:crypto";

import { parse } from "csv-parse/sync";
import {
  cached,
  fail,
  flatMap,
  gen,
  map,
  mapError,
  retry,
  succeed,
} from "effect/Effect";
import { HttpClient } from "effect/http";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";
import * as Result from "effect/Result";
import { fromArray, unwrap } from "effect/Stream";

import {
  hfFetchRetrySchedule,
  resolveHfToken,
} from "../../../datasets/huggingface";
import type { Sample } from "../../../harness/core";
import { DatasetError } from "../../../harness/core";
import type { Dataset } from "../../../harness/dataset";
import { Dataset as DatasetTag } from "../../../harness/dataset";
import { definedValues, isRecord } from "../../../internal/guards";
import { fetchHttpClientLayer } from "../../../internal/http-client";
import { parseSchema, z } from "../../../internal/zod";
import type { RetryConfig } from "../../../runtime/retry";

export const DSQA_DATASET_REVISION = "b2623f8653065c2672de6d941fc5434cd652376c";

export const DSQA_DATASET_URL = `https://huggingface.co/datasets/google/deepsearchqa/resolve/${DSQA_DATASET_REVISION}/DSQA-full.csv`;

export const DSQA_DATASET_SHA256 =
  "25d48dcf7efa872e5467032e8b8eedf38d301f59a252d0da95cda584baa78396";

export const DSQA_DATASET_ROWS = 900;

const DsqaRecordSchema = z.object({
  problem: z.string().min(1),
  answer: z.string().min(1),
  problem_category: z.string().min(1),
  answer_type: z.enum(["Single Answer", "Set Answer"]),
});

type DsqaRecord = z.infer<typeof DsqaRecordSchema>;

export function parseDsqaDataset(
  text: string,
  expectedSha256 = DSQA_DATASET_SHA256,
  expectedRows = DSQA_DATASET_ROWS
): Result.Result<readonly DsqaRecord[], string> {
  const digest = createHash("sha256").update(text, "utf8").digest("hex");
  if (digest !== expectedSha256) {
    return Result.fail(
      `DSQA checksum mismatch: expected ${expectedSha256}, got ${digest}`
    );
  }
  const parsedCsv = Result.try((): unknown =>
    parse(text, { columns: true, skip_empty_lines: true })
  );
  if (Result.isFailure(parsedCsv) || !Array.isArray(parsedCsv.success)) {
    return Result.fail("DSQA CSV failed to parse");
  }
  const records = parsedCsv.success
    .filter((record): record is Record<string, unknown> => isRecord(record))
    .map((record, index): Result.Result<DsqaRecord, string> => {
      const parsedRecord = parseSchema(DsqaRecordSchema, record);
      return Result.isFailure(parsedRecord)
        ? Result.fail(
            `DSQA row ${index} failed validation: ${parsedRecord.failure.message}`
          )
        : Result.succeed(parsedRecord.success);
    });
  const firstError = records.find(Result.isFailure);
  if (firstError !== undefined && Result.isFailure(firstError)) {
    return Result.fail(firstError.failure);
  }
  const data = records.filter(Result.isSuccess).map((record) => record.success);
  return data.length === expectedRows
    ? Result.succeed(data)
    : Result.fail(
        `DSQA row count drifted: expected ${expectedRows}, got ${data.length}`
      );
}

export function dsqaRecordToSample(record: DsqaRecord, index: number): Sample {
  return {
    id: `dsqa-${index}`,
    input: record.problem,
    target: { text: record.answer },
    metadata: {
      problem_category: record.problem_category,
      prompt_type: record.answer_type,
    },
  };
}

export function makeDsqaDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  const fetchRetry = hfFetchRetrySchedule(retryConfig);
  return effect(
    DatasetTag,
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      const hfToken = yield* resolveHfToken();
      const records = yield* cached(
        client
          .get(
            DSQA_DATASET_URL,
            definedValues({
              headers:
                hfToken !== ""
                  ? { Authorization: `Bearer ${hfToken}` }
                  : undefined,
            })
          )
          .pipe(
            flatMap((response) => response.text),
            retry(fetchRetry),
            mapError(
              (cause) =>
                new DatasetError({
                  message: `DSQA dataset fetch failed: ${String(cause)}`,
                })
            ),
            flatMap((text) => {
              const parsed = parseDsqaDataset(text);
              return Result.isFailure(parsed)
                ? fail(new DatasetError({ message: parsed.failure }))
                : succeed(parsed.success);
            })
          )
      );
      return DatasetTag.of({
        size: records.pipe(map((rows) => rows.length)),
        stream: (opts) =>
          unwrap(
            records.pipe(
              map((rows) => {
                const start = opts?.start ?? 0;
                const end = Math.min(opts?.end ?? rows.length, rows.length);
                return fromArray(
                  rows
                    .slice(start, end)
                    .map((record, offset) =>
                      dsqaRecordToSample(record, start + offset)
                    )
                );
              })
            )
          ),
      });
    })
  ).pipe(provide(fetchHttpClientLayer));
}
