import { FetchHttpClient } from "@effect/platform";
import { flatMap, provide } from "effect/Effect";
import {
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";

import type {
  BenchmarkRunConfig,
  InjectedBenchmarkRunConfig,
} from "../benchmarks/benchmark-config";
import {
  isNativeBenchmarkConfig,
  modelFromConfig,
} from "../benchmarks/benchmark-config";
import { getBenchmark } from "../benchmarks/registry";
import type {
  Benchmark,
  BenchmarkMetadata,
  BenchmarkRunInput,
} from "../benchmarks/types";
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
import type { SampleResultStoreService } from "../harness/sample-result-store";
import {
  namespacedSampleResultStore,
  NOOP_SAMPLE_RESULT_STORE,
  SampleResultStore,
} from "../harness/sample-result-store";
import { runHarnessPromise } from "../internal/effect-logger";
import type { AsyncEither } from "../internal/either";
import { Either } from "../internal/either";
import { wLog } from "../internal/log";
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
  readonly sampleResultStore?: SampleResultStoreService;
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
): AsyncEither<RunBenchmarkOutput, string> {
  const benchmarkResult = resolveRunBenchmark(input);
  if (Either.isLeft(benchmarkResult)) {
    return Promise.resolve(Either.left(benchmarkResult.left));
  }
  const { benchmark, benchmarkLayer } = benchmarkResult.right;
  const progressLayer = layerSucceed(
    ProgressReporter,
    input.progressReporter ?? NOOP_PROGRESS_REPORTER
  );
  const checkpointLayer = layerSucceed(
    CheckpointStore,
    input.checkpointStore ?? NOOP_CHECKPOINT_STORE
  );
  const sampleResultLayer = layerSucceed(
    SampleResultStore,
    input.sampleResultStore === undefined
      ? NOOP_SAMPLE_RESULT_STORE
      : namespacedSampleResultStore(input.sessionId, input.sampleResultStore)
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
      ...(input.runAttempt !== undefined && {
        run_attempt: `${input.runAttempt}`,
      }),
    },
  };
  const fullBenchmarkLayer = benchmarkLayer.pipe(
    layerProvide(FetchHttpClient.layer)
  );
  const traceHeaders = filterTraceHeaders(input.traceHeaders);
  const resolverLayer = layerSucceed(
    GenerationResolver,
    makeOpenRouterGenerationResolver({
      apiKey: input.apiKey,
      ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
      ...(traceHeaders !== undefined && { traceHeaders }),
    })
  );
  const layers = layerMergeAll(
    fullBenchmarkLayer,
    progressLayer,
    checkpointLayer,
    sampleResultLayer,
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
  benchmarkId: string,
  injectedBenchmark?: Benchmark<InjectedBenchmarkRunConfig>
): AsyncEither<number, string> {
  const benchmarkResult = resolveBenchmark(benchmarkId, injectedBenchmark);
  if (Either.isLeft(benchmarkResult)) {
    return Promise.resolve(Either.left(benchmarkResult.left));
  }
  const benchmark = benchmarkResult.right;
  const datasetLayer = benchmark.makeDatasetLayer();
  const program = Dataset.pipe(flatMap((d) => d.size));
  return runHarnessPromise(program.pipe(provide(datasetLayer)))
    .then((size) => Either.right(size))
    .catch((error) => Either.left(String(error)));
}

function resolveBenchmark(
  benchmarkId: string,
  injectedBenchmark: Benchmark<InjectedBenchmarkRunConfig> | undefined
): Either.Either<BenchmarkMetadata, string> {
  if (injectedBenchmark !== undefined) {
    return injectedBenchmark.id === benchmarkId
      ? Either.right(injectedBenchmark)
      : Either.left(
          `Benchmark id mismatch: requested "${benchmarkId}", supplied "${injectedBenchmark.id}"`
        );
  }
  const benchmark = getBenchmark(benchmarkId);
  return benchmark === undefined
    ? Either.left(`Unknown benchmark "${benchmarkId}"`)
    : Either.right(benchmark);
}

function resolveRunBenchmark(input: RunBenchmarkInput): Either.Either<
  {
    readonly benchmark: BenchmarkMetadata;
    readonly benchmarkLayer: ReturnType<Benchmark["makeLayer"]>;
  },
  string
> {
  if (isNativeBenchmarkConfig(input.benchmarkConfig)) {
    if (input.injectedBenchmark !== undefined) {
      return Either.left(
        `An injected benchmark cannot be supplied for native benchmark "${input.benchmarkId}"`
      );
    }
    const nativeBenchmark = getBenchmark(input.benchmarkId);
    if (nativeBenchmark === undefined) {
      return Either.left(`Unknown benchmark "${input.benchmarkId}"`);
    }
    return Either.right({
      benchmark: nativeBenchmark,
      benchmarkLayer: makeBenchmarkLayer(
        nativeBenchmark,
        input,
        input.benchmarkConfig
      ),
    });
  }
  if (input.injectedBenchmark === undefined) {
    return Either.left(
      `An injected benchmark is required for injected config "${input.benchmarkId}"`
    );
  }
  if (input.injectedBenchmark.id !== input.benchmarkId) {
    return Either.left(
      `Benchmark id mismatch: requested "${input.benchmarkId}", supplied "${input.injectedBenchmark.id}"`
    );
  }
  return Either.right({
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
    apiKey: input.apiKey,
    benchmarkConfig,
    ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
    ...(traceHeaders !== undefined && { traceHeaders }),
    sessionId: input.sessionId,
    ...(input.datasetRetry !== undefined && {
      datasetRetry: input.datasetRetry,
    }),
    ...(maxRetries !== undefined && { modelRetry: { maxRetries } }),
    ...(input.maxOutputTokensCeiling !== undefined && {
      maxOutputTokensCeiling: input.maxOutputTokensCeiling,
    }),
  };
  return benchmark.makeLayer(benchmarkInput);
}
