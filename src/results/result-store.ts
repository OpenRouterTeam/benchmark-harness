import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import { succeed } from "effect/Effect";

import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import { modelFromConfig } from "../benchmarks/benchmark-config";
import type { Benchmark } from "../benchmarks/types";
import type { RunResult } from "../harness/run";
import { runResultToParquet } from "./parquet";

export interface ResultStoreService {
  readonly write: (opts: {
    readonly result: RunResult;
    readonly benchmark: Benchmark;
    readonly benchmarkConfig: BenchmarkRunConfig;
    readonly epochs: number;
    readonly sessionId: string;
  }) => Effect<string | null>;
}

export class ResultStore extends Tag("@openrouter/bench-harness/result-store")<
  ResultStore,
  ResultStoreService
>() {}

export function makeLocalResultStore(opts: {
  readonly dir: string;
}): ResultStoreService {
  return {
    write: ({ result, benchmark, benchmarkConfig, epochs, sessionId }) => {
      const benchmarkId = benchmarkConfig.benchmarkId;
      const model = modelFromConfig(benchmarkConfig) ?? benchmarkId;
      const extraScores = benchmark.runLevelScores?.(result);
      const primaryScore = benchmark.primaryScore?.(result);
      const parquetBuffer = runResultToParquet({
        result,
        meta: {
          task: benchmarkId,
          model,
          epochs,
          temperature: benchmark.temperature,
          benchmarkConfig,
        },
        ...(extraScores !== undefined && { extraScores }),
        ...(primaryScore !== undefined && { primaryScore }),
      });
      const safeModel = model.replaceAll("/", "_");
      const filename = `${benchmarkId}-${safeModel}-${sessionId}.parquet`;
      const filepath = join(opts.dir, filename);
      mkdirSync(opts.dir, { recursive: true });
      writeFileSync(filepath, parquetBuffer);
      return succeed(filepath);
    },
  };
}
