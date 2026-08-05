import { FetchHttpClient } from "@effect/platform";
import { flatMap, provide } from "effect/Effect";
import {
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";

import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import { modelFromConfig } from "../benchmarks/benchmark-config";
import { getBenchmark } from "../benchmarks/registry";
import { Dataset } from "../harness/dataset";
import type {
  CheckpointStoreService,
  ProgressReporterService,
} from "../harness/progress";
import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../harness/progress";
import type { RunResult, RunConfig } from "../harness/run";
import { runBenchmark } from "../harness/run";
import { runHarnessPromise } from "../internal/effect-logger";
import type { AsyncEither } from "../internal/either";
import { Either } from "../internal/either";
import { wLog } from "../internal/log";
import type { ResultStoreService } from "../results/result-store";
import type { RetryConfig } from "../runtime/retry";

export interface RunBenchmarkInput {
  readonly benchmarkId: string;
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly benchmarkConfig: BenchmarkRunConfig;
  readonly epochs: number;
  readonly maxConcurrency: number;
  readonly range?: {
    readonly start?: number;
    readonly end?: number;
  };
  readonly sessionId: string;
  readonly datasetRetry?: RetryConfig;
  readonly progressReporter?: ProgressReporterService;
  readonly checkpointStore?: CheckpointStoreService;
  readonly abortSignal?: AbortSignal;
  readonly resultStore?: ResultStoreService;
}

export interface RunBenchmarkOutput {
  readonly result: RunResult;
  readonly resultsPath: string | null;
}

export function runBenchmarkById(
  input: RunBenchmarkInput
): AsyncEither<RunBenchmarkOutput, string> {
  const benchmark = getBenchmark(input.benchmarkId);
  if (benchmark === undefined) {
    return Promise.resolve(
      Either.left(`Unknown benchmark "${input.benchmarkId}"`)
    );
  }
  const maxRetries = input.benchmarkConfig.maxRetries;
  const benchmarkLayer = benchmark.makeLayer({
    apiKey: input.apiKey,
    benchmarkConfig: input.benchmarkConfig,
    ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
    sessionId: input.sessionId,
    ...(input.datasetRetry !== undefined && {
      datasetRetry: input.datasetRetry,
    }),
    ...(maxRetries !== undefined && { modelRetry: { maxRetries } }),
  });
  const progressLayer = layerSucceed(
    ProgressReporter,
    input.progressReporter ?? NOOP_PROGRESS_REPORTER
  );
  const checkpointLayer = layerSucceed(
    CheckpointStore,
    input.checkpointStore ?? NOOP_CHECKPOINT_STORE
  );
  const model = modelFromConfig(input.benchmarkConfig);
  const runConfig: RunConfig = {
    epochs: input.epochs,
    maxConcurrency: input.maxConcurrency,
    ...(input.range !== undefined && { range: input.range }),
    ...(benchmark.degradeSolverErrors !== undefined && {
      degradeSolverErrors: benchmark.degradeSolverErrors,
    }),
    logAnnotations: {
      benchmark: input.benchmarkId,
      session_id: input.sessionId,
      ...(model !== undefined && { model }),
    },
  };
  const fullBenchmarkLayer = benchmarkLayer.pipe(
    layerProvide(FetchHttpClient.layer)
  );
  const layers = layerMergeAll(
    fullBenchmarkLayer,
    progressLayer,
    checkpointLayer
  );
  const runOpts =
    input.abortSignal !== undefined ? { signal: input.abortSignal } : undefined;
  return runHarnessPromise(
    runBenchmark(runConfig).pipe(provide(layers)),
    runOpts
  )
    .then((result) => {
      if (input.resultStore !== undefined) {
        return runHarnessPromise(
          input.resultStore.write({
            result,
            benchmark,
            benchmarkConfig: input.benchmarkConfig,
            epochs: input.epochs,
            sessionId: input.sessionId,
          })
        )
          .then((resultsPath) => Either.right({ result, resultsPath }))
          .catch((storeErr) => {
            wLog("Failed to persist benchmark results", {
              error: String(storeErr),
            });
            return Either.right({ result, resultsPath: null });
          });
      }
      return Either.right({ result, resultsPath: null });
    })
    .catch((error) => Either.left(String(error)));
}

export function datasetSizeById(
  benchmarkId: string
): AsyncEither<number, string> {
  const benchmark = getBenchmark(benchmarkId);
  if (benchmark === undefined) {
    return Promise.resolve(Either.left(`Unknown benchmark "${benchmarkId}"`));
  }
  const datasetLayer = benchmark.makeDatasetLayer();
  const program = Dataset.pipe(flatMap((d) => d.size));
  return runHarnessPromise(program.pipe(provide(datasetLayer)))
    .then((size) => Either.right(size))
    .catch((error) => Either.left(String(error)));
}
