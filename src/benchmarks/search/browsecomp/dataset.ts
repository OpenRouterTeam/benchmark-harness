import { createHash } from "node:crypto";

import { parse } from "csv-parse/sync";
import {
  cached,
  gen,
  fail,
  flatMap,
  map,
  mapError,
  retry,
  succeed,
} from "effect/Effect";
import { HttpClient } from "effect/http";
import type { Layer } from "effect/Layer";
import { effect as layerEffect, provide as layerProvide } from "effect/Layer";
import * as Result from "effect/Result";
import { fromArray, unwrap } from "effect/Stream";

import { hfFetchRetrySchedule } from "../../../datasets/huggingface";
import type { Sample } from "../../../harness/core";
import { DatasetError } from "../../../harness/core";
import type { Dataset } from "../../../harness/dataset";
import { Dataset as DatasetTag } from "../../../harness/dataset";
import { isRecord } from "../../../internal/guards";
import { fetchHttpClientLayer } from "../../../internal/http-client";

export const BROWSECOMP_URL =
  "https://openaipublic.blob.core.windows.net/simple-evals/browse_comp_test_set.csv";

export const BROWSECOMP_SHA256 =
  "7b24471cd5b3eb2a46830a14802b5c029ea62f488ff75a0f88af7923d1454abf";

export const BROWSECOMP_ROWS = 1266;

export function decryptField(value: string, canary: string): string {
  const ciphertext = Buffer.from(value, "base64");
  const seed = createHash("sha256").update(canary, "utf8").digest();
  const plaintext = Buffer.from(
    Uint8Array.from(ciphertext, (byte, i) => byte ^ seed[i % seed.length]!)
  );
  return plaintext.toString("utf8");
}

function parseCsvRecords(
  text: string
): Result.Result<readonly Record<string, unknown>[], string> {
  const rows = Result.try((): unknown =>
    parse(text, { columns: true, skip_empty_lines: true })
  );
  if (Result.isFailure(rows)) {
    return Result.fail(`csv parse failed: ${String(rows.failure)}`);
  }
  if (!Array.isArray(rows.success)) {
    return Result.fail("csv-parse returned a non-array");
  }
  return Result.succeed(
    rows.success.filter((row): row is Record<string, unknown> => isRecord(row))
  );
}

function requireField(
  row: Readonly<Record<string, unknown>>,
  key: string,
  index: number
): Result.Result<string, string> {
  const value = row[key];
  if (typeof value !== "string" || value === "") {
    return Result.fail(
      `browsecomp row ${index} missing required field '${key}'`
    );
  }
  return Result.succeed(value);
}

export function browseCompRecordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number
): Result.Result<Sample, string> {
  const problem = requireField(record, "problem", index);
  if (Result.isFailure(problem)) {
    return Result.fail(problem.failure);
  }
  const answer = requireField(record, "answer", index);
  if (Result.isFailure(answer)) {
    return Result.fail(answer.failure);
  }
  const problemTopic = requireField(record, "problem_topic", index);
  if (Result.isFailure(problemTopic)) {
    return Result.fail(problemTopic.failure);
  }
  const canary = requireField(record, "canary", index);
  if (Result.isFailure(canary)) {
    return Result.fail(canary.failure);
  }
  return Result.succeed({
    id: `browsecomp-${index}`,
    input: decryptField(problem.success, canary.success),
    target: { text: decryptField(answer.success, canary.success) },
    metadata: { problem_topic: problemTopic.success, index },
  });
}

export function makeBrowseCompDatasetLayer(retryConfig?: {
  maxRetries?: number;
  baseDelayMs?: number;
}): Layer<Dataset> {
  const fetchRetry = hfFetchRetrySchedule(retryConfig);
  const layer = layerEffect(DatasetTag)(
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      const fetchCsv = yield* cached(
        client.get(BROWSECOMP_URL).pipe(
          flatMap((response) => response.text),
          retry(fetchRetry),
          mapError(
            (cause) =>
              new DatasetError({
                message: `browsecomp CSV fetch failed: ${String(cause)}`,
              })
          ),
          flatMap((text) => {
            const digest = createHash("sha256")
              .update(text, "utf8")
              .digest("hex");
            if (digest !== BROWSECOMP_SHA256) {
              return fail(
                new DatasetError({
                  message: `browsecomp CSV checksum mismatch: expected ${BROWSECOMP_SHA256}, got ${digest}`,
                })
              );
            }
            const parsed = parseCsvRecords(text);
            if (Result.isFailure(parsed)) {
              return fail(new DatasetError({ message: parsed.failure }));
            }
            if (parsed.success.length !== BROWSECOMP_ROWS) {
              return fail(
                new DatasetError({
                  message: `browsecomp row count drifted: expected ${BROWSECOMP_ROWS}, got ${parsed.success.length}`,
                })
              );
            }
            return succeed(parsed.success);
          })
        )
      );
      return DatasetTag.of({
        size: fetchCsv.pipe(map((rows) => rows.length)),
        stream: (opts) =>
          unwrap(
            fetchCsv.pipe(
              flatMap((rows) => {
                const start = opts?.start ?? 0;
                const end = Math.min(opts?.end ?? rows.length, rows.length);
                const mapped = rows
                  .slice(start, end)
                  .map((row, i) => browseCompRecordToSample(row, start + i));
                const firstError = mapped.find(Result.isFailure);
                if (firstError !== undefined && Result.isFailure(firstError)) {
                  return fail(
                    new DatasetError({ message: firstError.failure })
                  );
                }
                const samples = mapped
                  .filter(Result.isSuccess)
                  .map((r) => r.success);
                return succeed(fromArray(samples));
              })
            )
          ),
      });
    })
  );
  return layer.pipe(layerProvide(fetchHttpClientLayer));
}
