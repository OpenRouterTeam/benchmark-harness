import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import { map, succeed } from "effect/Effect";

import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import { modelFromConfig } from "../benchmarks/benchmark-config";
import type { BenchmarkMetadata } from "../benchmarks/types";
import type { RunResult } from "../harness/run";
import { definedValues } from "../internal/guards";
import type { ArtifactSink, ArtifactUploadError } from "./artifacts";
import { offloadInlineBase64 } from "./artifacts";
import { runResultToParquet } from "./parquet";

interface WriteOpts {
  readonly result: RunResult;
  readonly benchmark: BenchmarkMetadata;
  readonly benchmarkConfig: BenchmarkRunConfig;
  readonly epochs: number;
  readonly sessionId: string;
}

export interface ResultStoreService {
  readonly write: (
    opts: WriteOpts
  ) => Effect<string | null, ArtifactUploadError>;
}

export class ResultStore extends Tag("@openrouter/bench-harness/result-store")<
  ResultStore,
  ResultStoreService
>() {}

export function makeLocalResultStore(opts: {
  readonly dir: string;
  readonly artifactSink?: ArtifactSink;
}): ResultStoreService {
  const writeParquet = (
    { benchmark, benchmarkConfig, epochs, sessionId }: WriteOpts,
    result: RunResult
  ): string => {
    const benchmarkId = benchmarkConfig.benchmarkId;
    const model = modelFromConfig(benchmarkConfig) ?? benchmarkId;
    const extraScores = benchmark.runLevelScores?.(result);
    const primaryScore = benchmark.primaryScore?.(result);
    const parquetBuffer = runResultToParquet(
      definedValues({
        result,
        meta: {
          task: benchmarkId,
          model,
          epochs,
          temperature: benchmark.temperature,
          benchmarkConfig,
        },
        extraScores,
        primaryScore,
      })
    );
    const safeModel = model.replaceAll("/", "_");
    const filename = `${benchmarkId}-${safeModel}-${sessionId}.parquet`;
    const filepath = join(opts.dir, filename);
    mkdirSync(opts.dir, { recursive: true });
    writeFileSync(filepath, parquetBuffer);
    return filepath;
  };
  return {
    write: (writeOpts) =>
      (opts.artifactSink === undefined
        ? succeed(writeOpts.result)
        : offloadInlineBase64(writeOpts.result, opts.artifactSink)
      ).pipe(map((result) => writeParquet(writeOpts, result))),
  };
}
