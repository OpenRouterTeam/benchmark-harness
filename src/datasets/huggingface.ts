import { createHash } from "node:crypto";
import { join } from "node:path";

import { FetchHttpClient, HttpClient } from "@effect/platform";
import { fromIterable } from "effect/Chunk";
import { map as configMap, option, string } from "effect/Config";
import type { Effect } from "effect/Effect";
import {
  fail,
  flatMap,
  gen,
  map,
  mapError,
  orElseSucceed,
  promise,
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
import { paginateChunkEffect } from "effect/Stream";

import type { Sample } from "../harness/core";
import { DatasetError } from "../harness/core";
import type { DatasetStreamOptions } from "../harness/dataset";
import { Dataset } from "../harness/dataset";
import { Either } from "../internal/either";
import { definedValues } from "../internal/guards";
import { parseSchema, z } from "../internal/zod";
import type { RetryConfig } from "../runtime/retry";
import { withRetryAttemptLogging } from "../runtime/retry";
import type { CacheStore } from "./cache-store";
import { resolveCacheStore } from "./cache-store";
import { encodeCacheKeySegment, readEnvOptional } from "./local-cache";

const HF_MAX_PAGE_SIZE = 100;

const HF_ROWS_BASE_URL = "https://datasets-server.huggingface.co/rows";

export const HF_CACHE_TTL_ENV = "BENCH_HF_CACHE_TTL_MS";

export const HF_CACHE_DEFAULT_TTL_MS = 24 * 60 * 60 * 1e3;

export function resolveHfCacheTtlMs(): number | undefined {
  const raw = readEnvOptional(HF_CACHE_TTL_ENV);
  if (raw !== undefined) {
    const parsed = Number.parseInt(raw, 10);
    if (Number.isFinite(parsed) && parsed >= 0) {
      return parsed;
    }
  }
  return undefined;
}

function hfTokenCacheSegment(hfToken: string): string {
  if (hfToken === "") {
    return "anon";
  }
  return createHash("sha256").update(hfToken).digest("hex").slice(0, 16);
}

function hfPageCacheKey(
  config: HfDatasetConfig,
  offset: number,
  length: number,
  hfToken: string,
  store: CacheStore
): string | undefined {
  if (!store.enabled) {
    return undefined;
  }
  return join(
    "hf",
    hfTokenCacheSegment(hfToken),
    encodeCacheKeySegment(config.dataset),
    encodeCacheKeySegment(config.config),
    encodeCacheKeySegment(config.split),
    encodeCacheKeySegment(config.revision ?? "HEAD"),
    `${offset}-${length}.json`
  );
}

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
  readonly revision?: string;
  readonly cacheStore?: CacheStore;
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
  client: HttpClient.HttpClient,
  store: CacheStore
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
      const cacheKey = hfPageCacheKey(config, offset, length, hfToken, store);
      if (cacheKey !== undefined) {
        const explicitTtlMs = resolveHfCacheTtlMs();
        const maxAgeMs =
          explicitTtlMs ??
          (config.revision === undefined ? HF_CACHE_DEFAULT_TTL_MS : undefined);
        const cached = yield* promise(() =>
          store.readJson(
            cacheKey,
            maxAgeMs !== undefined ? { maxAgeMs } : undefined
          )
        );
        if (cached !== undefined) {
          const cachedParsed = parseSchema(HfRowsResponseSchema, cached);
          if (Either.isRight(cachedParsed)) {
            return cachedParsed.right;
          }
        }
      }
      const body = yield* client
        .get(HF_ROWS_BASE_URL, {
          urlParams: definedValues({
            dataset: config.dataset,
            config: config.config,
            split: config.split,
            offset,
            length,
            revision: config.revision,
          }),
          ...definedValues({
            headers:
              hfToken !== undefined && hfToken !== ""
                ? { Authorization: `Bearer ${hfToken}` }
                : undefined,
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
      if (cacheKey !== undefined) {
        yield* promise(() => store.writeJson(cacheKey, body));
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

export function makeHfDatasetLayer(config: HfDatasetConfig): Layer<Dataset> {
  const pageSize = Math.min(
    config.pageSize ?? HF_MAX_PAGE_SIZE,
    HF_MAX_PAGE_SIZE
  );
  const makeService = gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const store = config.cacheStore ?? resolveCacheStore();
    const fetchPage = makeHfPageFetcher(config, client, store);
    const sizeEffect: Effect<number, DatasetError> = fetchPage(0, 1).pipe(
      map((page) => page.num_rows_total)
    );
    const stream = (
      opts?: DatasetStreamOptions
    ): Stream<Sample, DatasetError> => {
      return paginateHfRows({
        fetchPage,
        pageSize,
        dataset: config.dataset,
        start: opts?.start,
        end: opts?.end,
        mapRow: (row, index) => config.recordToSample(row.row, index),
      });
    };
    return Dataset.of({ stream, size: sizeEffect });
  });
  return effect(Dataset, makeService).pipe(provide(FetchHttpClient.layer));
}
