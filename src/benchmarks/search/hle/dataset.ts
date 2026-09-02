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
  retry,
  succeed,
  tryPromise,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect } from "effect/Layer";
import type { Option } from "effect/Option";
import { getOrNull, none, some } from "effect/Option";
import { paginateChunkEffect } from "effect/Stream";

import type { HfRowsResponse } from "../../../datasets/huggingface";
import {
  HfRowsResponseSchema,
  hfFetchRetrySchedule,
} from "../../../datasets/huggingface";
import type { Sample } from "../../../harness/core";
import { DatasetError } from "../../../harness/core";
import type { Dataset } from "../../../harness/dataset";
import { Dataset as DatasetTag } from "../../../harness/dataset";
import { Either } from "../../../internal/either";
import { definedValues } from "../../../internal/guards";
import { parseSchema, z } from "../../../internal/zod";
import type { RetryConfig } from "../../../runtime/retry";

export const HLE_REPO = "cais/hle";

export const HLE_REVISION = "5a81a4c7271a2a2a312b9a690f0c2fde837e4c29";

export const HLE_ACCESS_URL = "https://huggingface.co/datasets/cais/hle";

export const HLE_TEXT_ONLY_ROWS = 2158;

export const HLE_TEXT_ONLY_WHERE = `"image".length()=0`;

const HleRecordSchema = z.object({
  id: z.string(),
  question: z.string(),
  image: z.literal(""),
  answer: z.string(),
  answer_type: z.string(),
  raw_subject: z.string(),
  category: z.string(),
});

interface PageState {
  readonly offset: number;
  readonly limit: number;
}

export function hleRecordToSample(
  record: Readonly<Record<string, unknown>>
): Either.Either<Sample, string> {
  const parsed = parseSchema(HleRecordSchema, record);
  if (Either.isLeft(parsed)) {
    return Either.left(`invalid HLE record: ${parsed.left.message}`);
  }
  return Either.right({
    id: parsed.right.id,
    input: parsed.right.question,
    target: { text: parsed.right.answer },
    metadata: {
      answer_type: parsed.right.answer_type,
      raw_subject: parsed.right.raw_subject,
      category: parsed.right.category,
    },
  });
}

export function makeHleDatasetLayer(
  retryConfig?: RetryConfig,
  hfToken?: string
): Layer<Dataset> {
  return effect(
    DatasetTag,
    gen(function* () {
      const token = yield* cached(
        hfToken !== undefined
          ? succeed(hfToken)
          : string("HF_TOKEN").pipe(
              option,
              configMap((value) => getOrNull(value) ?? ""),
              mapError(
                () =>
                  new DatasetError({
                    message: "Failed to read HF_TOKEN config",
                  })
              )
            )
      );
      const verifyRevision = yield* cached(
        token.pipe(flatMap((value) => fetchHleRevision(retryConfig, value)))
      );
      const fetchPage = (offset: number, length: number) =>
        verifyRevision.pipe(
          flatMap(() => token),
          flatMap((value) =>
            fetchHlePage({ offset, length, retryConfig, hfToken: value })
          )
        );
      return DatasetTag.of({
        size: fetchPage(0, 1).pipe(map((page) => page.num_rows_total)),
        stream: (opts) => {
          const start = opts?.start ?? 0;
          return paginateChunkEffect(
            { offset: start, limit: 100 },
            (state: PageState) =>
              fetchPage(state.offset, state.limit).pipe(
                flatMap((page) => {
                  const end = Math.min(
                    opts?.end ?? page.num_rows_total,
                    page.num_rows_total
                  );
                  const mapped = page.rows
                    .slice(0, Math.max(0, end - state.offset))
                    .map(({ row }) => hleRecordToSample(row));
                  const firstError = mapped.find(Either.isLeft);
                  if (firstError !== undefined && Either.isLeft(firstError)) {
                    return fail(new DatasetError({ message: firstError.left }));
                  }
                  const nextOffset = state.offset + page.rows.length;
                  const next: Option<PageState> =
                    page.rows.length > 0 && nextOffset < end
                      ? some({ offset: nextOffset, limit: state.limit })
                      : none();
                  return succeed([
                    fromIterable(
                      mapped
                        .filter(Either.isRight)
                        .map((sample) => sample.right)
                    ),
                    next,
                  ] as const);
                })
              )
          );
        },
      });
    })
  );
}

function fetchHleRevision(
  retryConfig: RetryConfig | undefined,
  hfToken: string
): Effect<void, DatasetError> {
  return fetchJson(
    `https://huggingface.co/api/datasets/${HLE_REPO}`,
    retryConfig,
    hfToken
  ).pipe(
    flatMap((body) => {
      const parsed = parseSchema(z.object({ sha: z.string() }), body);
      if (Either.isLeft(parsed)) {
        return fail(
          new DatasetError({
            message: `HLE metadata failed validation: ${parsed.left.message}`,
          })
        );
      }
      return parsed.right.sha === HLE_REVISION
        ? succeed(undefined)
        : fail(
            new DatasetError({
              message: `HLE revision mismatch: expected ${HLE_REVISION}, got ${parsed.right.sha}`,
            })
          );
    })
  );
}

function fetchHlePage({
  offset,
  length,
  retryConfig,
  hfToken,
}: {
  readonly offset: number;
  readonly length: number;
  readonly retryConfig: RetryConfig | undefined;
  readonly hfToken: string;
}): Effect<HfRowsResponse, DatasetError> {
  const url = new URL("https://datasets-server.huggingface.co/filter");
  Object.entries({
    dataset: HLE_REPO,
    config: "default",
    split: "test",
    where: HLE_TEXT_ONLY_WHERE,
    offset: String(offset),
    length: String(length),
  }).forEach(([key, value]) => url.searchParams.set(key, value));
  return fetchJson(url.toString(), retryConfig, hfToken).pipe(
    flatMap((body) => {
      const parsed = parseSchema(HfRowsResponseSchema, body);
      if (Either.isLeft(parsed)) {
        return fail(
          new DatasetError({
            message: `HLE rows failed validation: ${parsed.left.message}`,
          })
        );
      }
      return parsed.right.num_rows_total === HLE_TEXT_ONLY_ROWS
        ? succeed(parsed.right)
        : fail(
            new DatasetError({
              message: `HLE text-only row count drifted: expected ${HLE_TEXT_ONLY_ROWS}, got ${parsed.right.num_rows_total}`,
            })
          );
    })
  );
}

function fetchJson(
  url: string,
  retryConfig: RetryConfig | undefined,
  hfToken: string
): Effect<unknown, DatasetError> {
  return tryPromise({
    try: async (): Promise<unknown> => {
      const response = await fetch(
        url,
        definedValues({
          headers:
            hfToken !== "" ? { Authorization: `Bearer ${hfToken}` } : undefined,
        })
      );
      const body: unknown = await response.json();
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      return body;
    },
    catch: (cause) => new DatasetError({ message: String(cause) }),
  }).pipe(
    retry(hfFetchRetrySchedule(retryConfig)),
    mapError(
      (cause) =>
        new DatasetError({
          message: `${cause.message}. HLE is gated: accept the terms at ${HLE_ACCESS_URL} and provide HF_TOKEN.`,
        })
    )
  );
}
