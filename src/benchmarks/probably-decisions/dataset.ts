import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { FetchHttpClient } from "@effect/platform";
import { fromIterable } from "effect/Chunk";
import type { Effect } from "effect/Effect";
import {
  cached,
  fail,
  flatMap,
  map,
  mapError,
  provide,
  succeed,
  tryPromise,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect } from "effect/Layer";
import { none, some } from "effect/Option";
import type { Stream } from "effect/Stream";
import {
  flatMap as flatMapStream,
  fromEffect,
  paginateChunkEffect,
} from "effect/Stream";

import { fetchCachedTextFile } from "../../datasets/cached-file";
import type { Sample } from "../../harness/core";
import { DatasetError } from "../../harness/core";
import type {
  DatasetService,
  DatasetStreamOptions,
} from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import type { RetryConfig } from "../../runtime/retry";
import type { DecisionRecord, DecisionSampleMeta } from "./schema";
import {
  DecisionRecordSchema,
  DecisionSampleMetaSchema,
  PROBABLY_DECISIONS_ID,
} from "./schema";

const PAGE_SIZE = 50;

const SENTENCE_END = /(?<=[.!?])\s/;

export interface DecisionDatasetConfig {
  readonly datasetUrl: string;
  readonly retry?: RetryConfig;
}

export function leadOf(dossier: string): string {
  const [first] = dossier.trim().split(SENTENCE_END, 1);
  return (first ?? dossier).trim();
}

export function decisionRecordToSample(
  record: Readonly<Record<string, unknown>>
): Sample {
  const parsed = parseSchema(DecisionRecordSchema, record);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `${PROBABLY_DECISIONS_ID} record failed validation: ${parsed.left.message}`
    );
  }
  const row: DecisionRecord = parsed.right;
  const metadata: DecisionSampleMeta = {
    domain: row.domain,
    source: row.source,
    lead: leadOf(row.dossier),
    dossier: row.dossier,
    evidence: row.evidence,
    humanDecision: row.gold.human_decision,
    goldFacts: row.gold.facts ?? [],
  };
  return {
    id: `${PROBABLY_DECISIONS_ID}-${row.id}`,
    input: row.dossier,
    target: { text: row.gold.outcome },
    metadata,
  };
}

export function readDecisionSampleMeta(
  metadata: Readonly<Record<string, unknown>> | undefined
): DecisionSampleMeta | undefined {
  const parsed = parseSchema(DecisionSampleMetaSchema, metadata);
  return Either.isRight(parsed) ? parsed.right : undefined;
}

export function parseJsonlLines(text: string): readonly string[] {
  return text.split("\n").filter((line) => line.trim() !== "");
}

function parseLine(line: string, index: number): Sample {
  const json = Either.try((): unknown => JSON.parse(line));
  if (
    Either.isLeft(json) ||
    json.right === null ||
    typeof json.right !== "object"
  ) {
    throw new TypeError(
      `${PROBABLY_DECISIONS_ID} line ${index} is not a JSON object`
    );
  }
  return decisionRecordToSample({ ...json.right });
}

function loadText(config: DecisionDatasetConfig): Effect<string, DatasetError> {
  if (config.datasetUrl.startsWith("file:")) {
    return tryPromise({
      try: () => readFile(fileURLToPath(config.datasetUrl), "utf8"),
      catch: (error) =>
        new DatasetError({
          message: `Failed to read dataset file: ${String(error)}`,
        }),
    });
  }
  return fetchCachedTextFile(
    definedValues({ url: config.datasetUrl, retry: config.retry })
  ).pipe(
    provide(FetchHttpClient.layer),
    mapError(
      (error) =>
        new DatasetError({
          message: `Failed to fetch dataset: ${String(error)}`,
        })
    )
  );
}

export function makeDecisionDatasetLayer(
  config: DecisionDatasetConfig
): Layer<Dataset> {
  return effect(
    Dataset,
    map(cached(loadText(config).pipe(map(parseJsonlLines))), (lines) =>
      buildDatasetService(lines)
    )
  );
}

function buildDatasetService(
  lines: Effect<readonly string[], DatasetError>
): DatasetService {
  const size = lines.pipe(map((all) => all.length));
  const stream = (
    options?: DatasetStreamOptions
  ): Stream<Sample, DatasetError> =>
    fromEffect(lines).pipe(
      flatMapStream((all) => {
        const start = options?.start ?? 0;
        const end = Math.min(options?.end ?? all.length, all.length);
        return paginateChunkEffect(start, (offset) => {
          const stop = Math.min(offset + PAGE_SIZE, end);
          const page = Either.try(() =>
            all
              .slice(offset, stop)
              .map((line, i) => parseLine(line, offset + i))
          );
          if (Either.isLeft(page)) {
            return fail(new DatasetError({ message: String(page.left) }));
          }
          return succeed([
            fromIterable(page.right),
            stop < end ? some(stop) : none(),
          ] as const);
        });
      })
    );
  return { stream, size: size.pipe(flatMap((n) => succeed(n))) };
}
