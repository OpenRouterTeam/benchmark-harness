import { createHash } from "node:crypto";

import { parse } from "csv-parse/sync";
import {
  map as configMap,
  option,
  String as configString,
} from "effect/Config";
import type { Effect } from "effect/Effect";
import {
  all,
  cached,
  fail,
  flatMap,
  gen,
  map,
  mapError,
  retry,
  succeed,
} from "effect/Effect";
import type { HttpClient } from "effect/http";
import { HttpClient as HttpClientTag } from "effect/http";
import type { HttpClientResponse } from "effect/http/HttpClientResponse";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";
import { getOrNull } from "effect/Option";
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
import { withLiveEnvConfig } from "../../../internal/env-config";
import { definedValues, isRecord } from "../../../internal/guards";
import { fetchHttpClientLayer } from "../../../internal/http-client";
import { parseSchema, z } from "../../../internal/zod";
import type { RetryConfig } from "../../../runtime/retry";

export const WIDESEARCH_REPO = "ByteDance-Seed/WideSearch";

export const WIDESEARCH_REVISION = "6531a7e5b497d44c8912407e0cb3dc95bd98cc09";

export const WIDESEARCH_DATASET_URL = `https://huggingface.co/datasets/${WIDESEARCH_REPO}/resolve/${WIDESEARCH_REVISION}/widesearch.jsonl`;

export const WIDESEARCH_DATASET_SHA256 =
  "bba28ec51dce28fa617f82617d88fcd6bdd4cd4d7f0a4d70db07d7fa8a90bdf4";

export const WIDESEARCH_ROWS = 200;

const GOLD_FETCH_CONCURRENCY = 4;

interface GoldFetchFailure {
  readonly message: string;
  readonly retryable: boolean;
}

const WideSearchSourceRowSchema = z.object({
  instance_id: z.string().min(1),
  query: z.string(),
  evaluation: z.string(),
  language: z.string(),
});

const WideSearchEvaluationSchema = z.object({
  required: z.array(z.string()),
  unique_columns: z.array(z.string()),
  eval_pipeline: z.record(z.string(), z.unknown()),
});

export function parseWideSearchSource(
  text: string,
  expectedSha256 = WIDESEARCH_DATASET_SHA256,
  expectedRows = WIDESEARCH_ROWS
): Result.Result<readonly z.infer<typeof WideSearchSourceRowSchema>[], string> {
  const digest = createHash("sha256").update(text, "utf8").digest("hex");
  if (digest !== expectedSha256) {
    return Result.fail(
      `WideSearch checksum mismatch: expected ${expectedSha256}, got ${digest}`
    );
  }
  const rows = text
    .trimEnd()
    .split("\n")
    .map(
      (
        line,
        index
      ): Result.Result<z.infer<typeof WideSearchSourceRowSchema>, string> => {
        const json = Result.try(() => JSON.parse(line));
        if (Result.isFailure(json)) {
          return Result.fail(`WideSearch row ${index} is invalid JSON`);
        }
        const parsed = parseSchema(WideSearchSourceRowSchema, json.success);
        return Result.isFailure(parsed)
          ? Result.fail(
              `WideSearch row ${index} failed validation: ${parsed.failure.message}`
            )
          : Result.succeed(parsed.success);
      }
    );
  const firstError = rows.find(Result.isFailure);
  if (firstError !== undefined && Result.isFailure(firstError)) {
    return Result.fail(firstError.failure);
  }
  const data = rows.filter(Result.isSuccess).map((row) => row.success);
  return data.length === expectedRows
    ? Result.succeed(data)
    : Result.fail(
        `WideSearch row count drifted: expected ${expectedRows}, got ${data.length}`
      );
}

function normalizeColumn(value: string): string {
  return value.trim().toLowerCase().replaceAll(" ", "");
}

export function parseWideSearchGoldCsv(
  source: string,
  taskId: string,
  required: readonly string[]
): Result.Result<readonly Readonly<Record<string, string>>[], string> {
  const parsed = Result.try((): unknown =>
    parse(source, { bom: true, columns: true, skip_empty_lines: true })
  );
  if (Result.isFailure(parsed) || !Array.isArray(parsed.success)) {
    return Result.fail(`WideSearch task ${taskId} gold CSV failed to parse`);
  }
  const rows = parsed.success.filter((row): row is Record<string, unknown> =>
    isRecord(row)
  );
  if (rows.length === 0) {
    return Result.fail(`WideSearch task ${taskId} gold CSV is empty`);
  }
  const headers = new Map(
    Object.keys(rows[0]!).map((header) => [normalizeColumn(header), header])
  );
  const missing = required.filter(
    (column) => !headers.has(normalizeColumn(column))
  );
  if (missing.length > 0) {
    return Result.fail(
      `WideSearch task ${taskId} gold CSV is missing required columns: ${JSON.stringify(missing)}`
    );
  }
  return Result.succeed(
    rows.map((row) =>
      Object.fromEntries(
        required.map((column) => [
          column,
          String(row[headers.get(normalizeColumn(column))!] ?? ""),
        ])
      )
    )
  );
}

function wideSearchRecordToSample({
  record,
  client,
  retryConfig,
}: {
  record: Readonly<Record<string, unknown>>;
  client: HttpClient.HttpClient;
  retryConfig?: RetryConfig;
}): Effect<Sample, DatasetError> {
  const source = parseSchema(WideSearchSourceRowSchema, record);
  if (Result.isFailure(source)) {
    return fail(
      new DatasetError({
        message: `WideSearch source row failed validation: ${source.failure.message}`,
      })
    );
  }
  const evaluationJson = Result.try(() =>
    JSON.parse(source.success.evaluation)
  );
  if (Result.isFailure(evaluationJson)) {
    return fail(
      new DatasetError({
        message: `WideSearch task ${source.success.instance_id} evaluation is invalid JSON`,
      })
    );
  }
  const evaluation = parseSchema(
    WideSearchEvaluationSchema,
    evaluationJson.success
  );
  if (Result.isFailure(evaluation)) {
    return fail(
      new DatasetError({
        message: `WideSearch task ${source.success.instance_id} evaluation failed validation: ${evaluation.failure.message}`,
      })
    );
  }
  const taskId = source.success.instance_id;
  return fetchWideSearchGoldCsv(client, taskId, retryConfig).pipe(
    flatMap((csv) => {
      const groundTruth = parseWideSearchGoldCsv(
        csv,
        taskId,
        evaluation.success.required
      );
      if (Result.isFailure(groundTruth)) {
        return fail(new DatasetError({ message: groundTruth.failure }));
      }
      return succeed({
        id: taskId,
        input: source.success.query,
        target: {
          text: JSON.stringify({
            ground_truth: groundTruth.success,
            evaluation: evaluation.success,
          }),
        },
        metadata: { language: source.success.language },
      });
    })
  );
}

export function makeWideSearchDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  const fetchRetry = hfFetchRetrySchedule(retryConfig);
  return effect(
    DatasetTag,
    gen(function* () {
      const client = yield* HttpClientTag.HttpClient;
      const hfToken = yield* resolveHfToken();
      const records = yield* cached(
        client
          .get(
            WIDESEARCH_DATASET_URL,
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
                  message: `WideSearch dataset fetch failed: ${String(cause)}`,
                })
            ),
            flatMap((text) => {
              const parsed = parseWideSearchSource(text);
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
              flatMap((rows) => {
                const start = opts?.start ?? 0;
                const end = Math.min(opts?.end ?? rows.length, rows.length);
                return all(
                  rows
                    .slice(start, end)
                    .map((record) =>
                      wideSearchRecordToSample({ record, client, retryConfig })
                    ),
                  { concurrency: GOLD_FETCH_CONCURRENCY }
                ).pipe(map((samples) => fromArray(samples)));
              })
            )
          ),
      });
    })
  ).pipe(provide(fetchHttpClientLayer));
}

function fetchWideSearchGoldCsv(
  client: HttpClient.HttpClient,
  taskId: string,
  retryConfig?: RetryConfig
): Effect<string, DatasetError> {
  const goldUrl = `https://huggingface.co/datasets/${WIDESEARCH_REPO}/resolve/${WIDESEARCH_REVISION}/widesearch_gold/${encodeURIComponent(taskId)}.csv`;
  return resolveGoldHfToken().pipe(
    flatMap((hfToken) =>
      client
        .get(
          goldUrl,
          definedValues({
            headers:
              hfToken !== ""
                ? { Authorization: `Bearer ${hfToken}` }
                : undefined,
          })
        )
        .pipe(
          mapError((cause): GoldFetchFailure => ({
            message: `request failed: ${String(cause)}`,
            retryable: true,
          })),
          flatMap((response) => goldResponseBody(response, taskId)),
          retry(
            hfFetchRetrySchedule(
              retryConfig,
              (failure: GoldFetchFailure): boolean => failure.retryable
            )
          )
        )
    ),
    mapError(
      (failure) =>
        new DatasetError({
          message: `Unable to download WideSearch gold CSV for task ${taskId}: ${failure.message}`,
        })
    )
  );
}

function goldResponseBody(
  response: HttpClientResponse,
  taskId: string
): Effect<string, GoldFetchFailure> {
  return response.text.pipe(
    mapError((cause): GoldFetchFailure => ({
      message: `failed to read gold CSV response body: ${String(cause)}`,
      retryable: true,
    })),
    flatMap((body) => {
      if (response.status >= 200 && response.status < 300) {
        return succeed(body);
      }
      const isRetryable = response.status === 429 || response.status >= 500;
      const prefix =
        response.status === 404
          ? `gold CSV not found for ${taskId}`
          : "gold CSV request failed";
      return fail({
        message: `${prefix} (HTTP ${response.status}): ${body}`,
        retryable: isRetryable,
      } satisfies GoldFetchFailure);
    })
  );
}

function resolveGoldHfToken(): Effect<string, GoldFetchFailure> {
  return withLiveEnvConfig(
    configString("HF_TOKEN").pipe(
      option,
      configMap((value) => getOrNull(value) ?? ""),
      mapError((): GoldFetchFailure => ({
        message: "failed to read HF_TOKEN config",
        retryable: false,
      }))
    )
  );
}
