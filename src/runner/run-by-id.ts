import { flatMap, provide } from "effect/Effect";
import {
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";
import * as Result from "effect/Result";
import { runCount } from "effect/Stream";

import type {
  BenchmarkRunConfig,
  InjectedBenchmarkRunConfig,
} from "../benchmarks/benchmark-config";
import {
  isKeplerBenchmarkConfig,
  modelFromConfig,
} from "../benchmarks/benchmark-config";
import { getBenchmark } from "../benchmarks/registry";
import type {
  Benchmark,
  BenchmarkMetadata,
  BenchmarkRunInput,
} from "../benchmarks/types";
import { datasetCacheWriteFailures } from "../datasets/cache-store";
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
import { definedValues } from "../internal/guards";
import { fetchHttpClientLayer } from "../internal/http-client";
import { wLog } from "../internal/log";
import type { AsyncResult } from "../internal/result";
import type { ResultStoreService } from "../results/result-store";
import {
  GenerationResolver,
  makeOpenRouterGenerationResolver,
} from "../runtime/generation-resolver";
import { withRunAttempt } from "../runtime/response-cache";
import type { RetryConfig } from "../runtime/retry";
import { filterTraceHeaders } from "./trace-headers";

export interface RunBenchmarkInput {
  readonly benchmarkId: string;
  readonly injectedBenchmark?: Benchmark<InjectedBenchmarkRunConfig>;
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
  readonly runAttempt?: number;
  readonly datasetRetry?: RetryConfig;
  readonly progressReporter?: ProgressReporterService;
  readonly checkpointStore?: CheckpointStoreService;
  readonly abortSignal?: AbortSignal;
  readonly resultStore?: ResultStoreService;
  readonly maxOutputTokensCeiling?: number;
  readonly traceHeaders?: Readonly<Record<string, string>>;
}

export interface RunBenchmarkOutput {
  readonly result: RunResult;
  readonly resultsPath: string | null;
}

export function runBenchmarkById(
  input: RunBenchmarkInput
): AsyncResult<RunBenchmarkOutput, string> {
  const benchmarkResult = resolveRunBenchmark(input);
  if (Result.isFailure(benchmarkResult)) {
    return Promise.resolve(Result.fail(benchmarkResult.failure));
  }
  const { benchmark, benchmarkLayer } = benchmarkResult.success;
  const progressLayer = layerSucceed(
    ProgressReporter,
    input.progressReporter ?? NOOP_PROGRESS_REPORTER
  );
  const checkpointLayer = layerSucceed(
    CheckpointStore,
    input.checkpointStore ?? NOOP_CHECKPOINT_STORE
  );
  const model = modelFromConfig(input.benchmarkConfig);
  const runConfig: RunConfig = definedValues({
    epochs: input.epochs,
    maxConcurrency: input.maxConcurrency,
    range: input.range,
    degradeSolverErrors: benchmark.degradeSolverErrors,
    logAnnotations: definedValues({
      benchmark: input.benchmarkId,
      session_id: input.sessionId,
      model,
      run_attempt:
        input.runAttempt !== undefined ? `${input.runAttempt}` : undefined,
    }),
  });
  const fullBenchmarkLayer = benchmarkLayer.pipe(
    layerProvide(fetchHttpClientLayer)
  );
  const traceHeaders = filterTraceHeaders(input.traceHeaders);
  const resolverLayer = layerSucceed(
    GenerationResolver,
    makeOpenRouterGenerationResolver(
      definedValues({
        apiKey: input.apiKey,
        baseUrl: input.baseUrl,
        traceHeaders,
      })
    )
  );
  const layers = layerMergeAll(
    fullBenchmarkLayer,
    progressLayer,
    checkpointLayer,
    resolverLayer
  );
  const runOpts =
    input.abortSignal !== undefined ? { signal: input.abortSignal } : undefined;
  const program = runBenchmark(runConfig).pipe(provide(layers));
  return runHarnessPromise(
    input.runAttempt === undefined
      ? program
      : withRunAttempt(input.runAttempt, program),
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
          .then((resultsPath) => Result.succeed({ result, resultsPath }))
          .catch((storeErr) => {
            wLog("Failed to persist benchmark results", {
              error: String(storeErr),
            });
            return Result.succeed({ result, resultsPath: null });
          });
      }
      return Result.succeed({ result, resultsPath: null });
    })
    .catch((error) => Result.fail(String(error)));
}

export function datasetSizeById(input: {
  readonly benchmarkId: string;
  readonly benchmarkConfig?: BenchmarkRunConfig;
  readonly injectedBenchmark?: Benchmark<InjectedBenchmarkRunConfig>;
}): AsyncResult<number, string> {
  const benchmarkResult = resolveBenchmark(
    input.benchmarkId,
    input.injectedBenchmark
  );
  if (Result.isFailure(benchmarkResult)) {
    return Promise.resolve(Result.fail(benchmarkResult.failure));
  }
  const benchmark = benchmarkResult.success;
  const datasetLayer =
    input.benchmarkConfig === undefined
      ? benchmark.makeDatasetLayer()
      : (benchmark.makeDatasetLayerForConfig?.(input.benchmarkConfig) ??
        benchmark.makeDatasetLayer());
  const program = Dataset.pipe(flatMap((d) => d.size));
  return runHarnessPromise(program.pipe(provide(datasetLayer)))
    .then((size) => Result.succeed(size))
    .catch((error) => Result.fail(String(error)));
}

export function warmDatasetById(input: {
  readonly benchmarkId: string;
  readonly benchmarkConfig: BenchmarkRunConfig;
  readonly range: { readonly start: number; readonly end: number };
  readonly datasetRetry?: RetryConfig;
  readonly injectedBenchmark?: Benchmark<InjectedBenchmarkRunConfig>;
}): AsyncResult<number, string> {
  const benchmarkResult = resolveBenchmark(
    input.benchmarkId,
    input.injectedBenchmark
  );
  if (Result.isFailure(benchmarkResult)) {
    return Promise.resolve(Result.fail(benchmarkResult.failure));
  }
  const failuresBefore = datasetCacheWriteFailures();
  const datasetLayer =
    benchmarkResult.success.makeDatasetLayerForConfig?.(
      input.benchmarkConfig,
      input.datasetRetry
    ) ?? benchmarkResult.success.makeDatasetLayer(input.datasetRetry);
  const program = Dataset.pipe(
    flatMap((dataset) => runCount(dataset.stream(input.range)))
  );
  return runHarnessPromise(program.pipe(provide(datasetLayer)))
    .then((count) => {
      const failures = datasetCacheWriteFailures() - failuresBefore;
      return failures > 0
        ? Result.fail(`dataset cache warmup: ${failures} cache write(s) failed`)
        : Result.succeed(count);
    })
    .catch((error) => Result.fail(String(error)));
}

function resolveBenchmark(
  benchmarkId: string,
  injectedBenchmark: Benchmark<InjectedBenchmarkRunConfig> | undefined
): Result.Result<BenchmarkMetadata, string> {
  if (injectedBenchmark !== undefined) {
    return injectedBenchmark.id === benchmarkId
      ? Result.succeed(injectedBenchmark)
      : Result.fail(
          `Benchmark id mismatch: requested "${benchmarkId}", supplied "${injectedBenchmark.id}"`
        );
  }
  const benchmark = getBenchmark(benchmarkId);
  return benchmark === undefined
    ? Result.fail(`Unknown benchmark "${benchmarkId}"`)
    : Result.succeed(benchmark);
}

function resolveRunBenchmark(input: RunBenchmarkInput): Result.Result<
  {
    readonly benchmark: BenchmarkMetadata;
    readonly benchmarkLayer: ReturnType<Benchmark["makeLayer"]>;
  },
  string
> {
  if (isKeplerBenchmarkConfig(input.benchmarkConfig)) {
    if (input.injectedBenchmark !== undefined) {
      return Result.fail(
        `An injected benchmark cannot be supplied for Kepler benchmark "${input.benchmarkId}"`
      );
    }
    const keplerBenchmark = getBenchmark(input.benchmarkId);
    if (keplerBenchmark === undefined) {
      return Result.fail(`Unknown benchmark "${input.benchmarkId}"`);
    }
    return Result.succeed({
      benchmark: keplerBenchmark,
      benchmarkLayer: makeBenchmarkLayer(
        keplerBenchmark,
        input,
        input.benchmarkConfig
      ),
    });
  }
  if (input.injectedBenchmark === undefined) {
    return Result.fail(
      `An injected benchmark is required for injected config "${input.benchmarkId}"`
    );
  }
  if (input.injectedBenchmark.id !== input.benchmarkId) {
    return Result.fail(
      `Benchmark id mismatch: requested "${input.benchmarkId}", supplied "${input.injectedBenchmark.id}"`
    );
  }
  return Result.succeed({
    benchmark: input.injectedBenchmark,
    benchmarkLayer: makeBenchmarkLayer(
      input.injectedBenchmark,
      input,
      input.benchmarkConfig
    ),
  });
}

function makeBenchmarkLayer<Config extends BenchmarkRunConfig>(
  benchmark: Benchmark<Config>,
  input: RunBenchmarkInput,
  benchmarkConfig: Config
): ReturnType<Benchmark["makeLayer"]> {
  const maxRetries = benchmarkConfig.maxRetries;
  const traceHeaders = filterTraceHeaders(input.traceHeaders);
  const benchmarkInput: BenchmarkRunInput<Config> = {
    benchmarkConfig,
    ...definedValues({
      apiKey: input.apiKey,
      baseUrl: input.baseUrl,
      traceHeaders,
      sessionId: input.sessionId,
      datasetRetry: input.datasetRetry,
      modelRetry: maxRetries !== undefined ? { maxRetries } : undefined,
      maxOutputTokensCeiling: input.maxOutputTokensCeiling,
    }),
  };
  return benchmark.makeLayer(benchmarkInput);
}
