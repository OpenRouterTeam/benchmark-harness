import { FetchHttpClient, HttpClient } from "@effect/platform";
import { fromIterable } from "effect/Chunk";
import { map as configMap, option, string } from "effect/Config";
import type { Effect } from "effect/Effect";
import {
  cached,
  fail,
  flatMap,
  gen,
  map,
  mapError,
  orElseSucceed,
  retry,
  succeed,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";
import type { Option } from "effect/Option";
import { getOrNull, none, some } from "effect/Option";
import type { Schedule } from "effect/Schedule";
import {
  exponential,
  jittered,
  passthrough,
  whileInput,
} from "effect/Schedule";
import type { Stream } from "effect/Stream";
import {
  flatMap as streamFlatMap,
  fromEffect,
  paginateChunkEffect,
} from "effect/Stream";

import type { Sample } from "../harness/core";
import { DatasetError } from "../harness/core";
import type { DatasetStreamOptions } from "../harness/dataset";
import { Dataset } from "../harness/dataset";
import { Either } from "../internal/either";
import { parseSchema, z } from "../internal/zod";
import type { RetryConfig } from "../runtime/retry";
import { withRetryAttemptLogging } from "../runtime/retry";

const HF_MAX_PAGE_SIZE = 100;

const HF_ROWS_BASE_URL = "https://datasets-server.huggingface.co/rows";
const HF_DATASET_INFO_BASE_URL = "https://huggingface.co/api/datasets";

export function hfFetchRetrySchedule<E = unknown>(
  config: RetryConfig = {},
  isRetryable: (error: E) => boolean = () => true
): Schedule<
  {
    readonly error: E;
    readonly attempt: number;
  },
  E
> {
  const maxRetries = config.maxRetries ?? 3;
  const baseDelayMs = config.baseDelayMs ?? 1e3;
  const scheduled = exponential(`${baseDelayMs} millis`).pipe(
    jittered,
    passthrough
  );
  return withRetryAttemptLogging(
    whileInput(scheduled, isRetryable).pipe(passthrough),
    maxRetries
  );
}

export function resolveHfToken(): Effect<string, never> {
  return string("HF_TOKEN").pipe(
    option,
    configMap((value) => getOrNull(value) ?? ""),
    orElseSucceed(() => "")
  );
}

export interface HfDatasetConfig {
  readonly dataset: string;
  readonly config: string;
  readonly split: string;
  readonly recordToSample: (
    record: Readonly<Record<string, unknown>>,
    index: number
  ) => Sample;
  readonly pageSize?: number;
  readonly retry?: RetryConfig;
  readonly hfToken?: string;
  /**
   * Expected dataset git revision (commit SHA). The Dataset Viewer /rows
   * endpoint only serves the default branch, so pinning is enforced by
   * VERIFICATION: before streaming, the dataset's current revision is fetched
   * and compared; a mismatch fails closed with the observed SHA so the
   * mismatch is explicit rather than silently scoring a different dataset.
   */
  readonly revision?: string;
}

export const HfImageSchema = z.object({
  src: z.string(),
  height: z.number().optional(),
  width: z.number().optional(),
});

export const HfRowsResponseSchema = z.object({
  rows: z.array(
    z.object({
      row_idx: z.number().int(),
      row: z.record(z.string(), z.unknown()),
    })
  ),
  num_rows_total: z.number().int(),
});

interface PageState {
  readonly offset: number;
  readonly limit: number;
}

export type HfRowsResponse = z.infer<typeof HfRowsResponseSchema>;

export type HfRow = HfRowsResponse["rows"][number];

export type HfPageFetcher = (
  offset: number,
  length: number
) => Effect<HfRowsResponse, DatasetError>;

export function makeHfPageFetcher(
  config: HfDatasetConfig,
  client: HttpClient.HttpClient
): HfPageFetcher {
  const fetchRetry = hfFetchRetrySchedule(config.retry);
  const hfTokenOverride = config.hfToken;
  return (offset, length) =>
    gen(function* () {
      const hfToken =
        hfTokenOverride !== undefined
          ? hfTokenOverride
          : yield* string("HF_TOKEN").pipe(
              option,
              configMap((opt) => getOrNull(opt) ?? ""),
              mapError(
                () =>
                  new DatasetError({
                    message: "Failed to read HF_TOKEN config",
                  })
              )
            );
      const body = yield* client
        .get(HF_ROWS_BASE_URL, {
          urlParams: {
            dataset: config.dataset,
            config: config.config,
            split: config.split,
            offset,
            length,
          },
          ...(hfToken !== undefined &&
            hfToken !== "" && {
              headers: { Authorization: `Bearer ${hfToken}` },
            }),
        })
        .pipe(
          flatMap((response) => response.json),
          retry(fetchRetry),
          mapError(
            (cause) =>
              new DatasetError({
                message: `HF /rows request failed (offset=${offset}): ${String(cause)}`,
              })
          )
        );
      const parsed = parseSchema(HfRowsResponseSchema, body);
      if (Either.isLeft(parsed)) {
        return yield* fail(
          new DatasetError({
            message: `HF /rows response failed validation (offset=${offset}): ${parsed.left.message}`,
          })
        );
      }
      return parsed.right;
    });
}

export function paginateHfRows<T>(opts: {
  readonly fetchPage: HfPageFetcher;
  readonly pageSize: number;
  readonly dataset?: string;
  readonly start?: number;
  readonly end?: number;
  readonly mapRow: (row: HfRow, index: number) => T;
}): Stream<T, DatasetError> {
  const start = opts.start ?? 0;
  const requestedEnd = opts.end;
  const initialState: PageState = { offset: start, limit: opts.pageSize };
  return paginateChunkEffect(initialState, (state: PageState) =>
    opts.fetchPage(state.offset, state.limit).pipe(
      flatMap((page) => {
        const end =
          requestedEnd !== undefined
            ? Math.min(page.num_rows_total, requestedEnd)
            : page.num_rows_total;
        const inRange = page.rows.filter(
          (r) => r.row_idx >= start && r.row_idx < end
        );
        const mapped = Either.try(() =>
          inRange.map((r) => opts.mapRow(r, r.row_idx))
        );
        if (Either.isLeft(mapped)) {
          return fail(
            new DatasetError({
              message: `Failed to map ${opts.dataset ?? "HF"} record(s) at offset ${state.offset}: ${String(mapped.left)}`,
            })
          );
        }
        const nextOffset = state.offset + page.rows.length;
        const hasMore = page.rows.length > 0 && nextOffset < end;
        const next: Option<PageState> = hasMore
          ? some({ offset: nextOffset, limit: opts.pageSize })
          : none();
        return succeed([fromIterable(mapped.right), next] as const);
      })
    )
  );
}

/**
 * Build a Dataset Layer backed by the HF Dataset Viewer /rows API. The stream
 * is paginated and backpressured: pages are fetched only as the consumer pulls,
 * so peak memory is one page plus whatever the run pipeline holds in flight.
 */
const HfDatasetInfoSchema = z.object({ sha: z.string() });

/**
 * Fail closed when the dataset's current default-branch revision differs from
 * the pinned one. The Dataset Viewer /rows endpoint always serves the default
 * branch, so this check is what turns `revision` from documentation into an
 * enforced comparability guarantee: an upstream dataset push (e.g. label
 * fixes) fails the run with both SHAs instead of silently scoring different
 * data under an unchanged identity digest.
 */
export function verifyHfRevision(
  config: Pick<HfDatasetConfig, "dataset" | "revision" | "retry">,
  client: HttpClient.HttpClient
): Effect<void, DatasetError> {
  const pinned = config.revision;
  if (pinned === undefined) {
    return succeed(undefined);
  }
  const fetchRetry = hfFetchRetrySchedule(config.retry);
  return client.get(`${HF_DATASET_INFO_BASE_URL}/${config.dataset}`).pipe(
    flatMap((response) => response.json),
    retry(fetchRetry),
    mapError(
      (cause) =>
        new DatasetError({
          message: `HF dataset-info request failed for ${config.dataset}: ${String(cause)}`,
        })
    ),
    flatMap((body) => {
      const parsed = parseSchema(HfDatasetInfoSchema, body);
      if (Either.isLeft(parsed)) {
        return fail(
          new DatasetError({
            message: `HF dataset-info response failed validation for ${config.dataset}: ${parsed.left.message}`,
          })
        );
      }
      if (parsed.right.sha !== pinned) {
        return fail(
          new DatasetError({
            message:
              `HF dataset ${config.dataset} revision mismatch: pinned ${pinned}, ` +
              `upstream default branch is at ${parsed.right.sha}. The dataset changed ` +
              `upstream; re-pin the revision (new comparability series) or investigate.`,
          })
        );
      }
      return succeed(undefined);
    })
  );
}

export function makeHfDatasetLayer(config: HfDatasetConfig): Layer<Dataset> {
  const pageSize = Math.min(
    config.pageSize ?? HF_MAX_PAGE_SIZE,
    HF_MAX_PAGE_SIZE
  );
  const makeService = gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const fetchPage = makeHfPageFetcher(config, client);
    /* Verification runs inside size/stream — the Dataset error channel
     * already carries DatasetError, while layer construction must stay
     * infallible. `cached` memoizes the check so one run makes at most one
     * dataset-info request no matter how many size/stream calls follow. */
    const verified = yield* cached(verifyHfRevision(config, client));

    const sizeEffect: Effect<number, DatasetError> = verified.pipe(
      flatMap(() => fetchPage(0, 1)),
      map((page) => page.num_rows_total)
    );
    const stream = (
      opts?: DatasetStreamOptions
    ): Stream<Sample, DatasetError> => {
      return fromEffect(verified).pipe(
        streamFlatMap(() =>
          paginateHfRows({
            fetchPage,
            pageSize,
            dataset: config.dataset,
            start: opts?.start,
            end: opts?.end,
            mapRow: (row, index) => config.recordToSample(row.row, index),
          })
        )
      );
    };
    return Dataset.of({ stream, size: sizeEffect });
  });
  return effect(Dataset, makeService).pipe(provide(FetchHttpClient.layer));
}
