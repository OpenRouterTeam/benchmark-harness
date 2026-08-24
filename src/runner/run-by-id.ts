import { FetchHttpClient } from "@effect/platform";
import { flatMap, provide } from "effect/Effect";
import {
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";

import type {
  BenchmarkRunConfig,
  HostBenchmarkRunConfig,
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
import type { RunResult, RunConfig, SampleOutcome } from "../harness/run";
import {
  aggregateOutcomes,
  runBenchmark,
  sampleEpochKey,
} from "../harness/run";
import { runHarnessPromise } from "../internal/effect-logger";
import type { AsyncEither } from "../internal/either";
import { Either } from "../internal/either";
import { wLog } from "../internal/log";
import type {
  PartialOutcomeRunScope,
  PartialOutcomesPayload,
  PartialOutcomeStoreService,
} from "../results/partial-outcome-store";
import { isSameRunScope } from "../results/partial-outcome-store";
import type { ResultStoreService } from "../results/result-store";
import {
  GenerationResolver,
  makeOpenRouterGenerationResolver,
} from "../runtime/generation-resolver";
import { withRunAttempt } from "../runtime/response-cache";
import type { RetryConfig } from "../runtime/retry";

export interface RunBenchmarkInput {
  readonly benchmarkId: string;
  readonly hostBenchmark?: Benchmark<HostBenchmarkRunConfig>;
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
  readonly partialOutcomeStore?: PartialOutcomeStoreService;
  readonly maxOutputTokensCeiling?: number;
}

export interface RunBenchmarkOutput {
  readonly result: RunResult;
  readonly resultsPath: string | null;
}

export async function runBenchmarkById(
  input: RunBenchmarkInput
): AsyncEither<RunBenchmarkOutput, string> {
  const benchmarkResult = resolveRunBenchmark(input);
  if (Either.isLeft(benchmarkResult)) {
    return Either.left(benchmarkResult.left);
  }
  const { benchmark, benchmarkLayer } = benchmarkResult.right;
  const partialStore = input.partialOutcomeStore;
  const runScope: PartialOutcomeRunScope = {
    epochs: input.epochs,
    ...(input.range !== undefined && { range: input.range }),
  };
  const priorOutcomes =
    partialStore === undefined
      ? []
      : await readPartialOutcomes(partialStore, runScope);
  const collectedOutcomes: SampleOutcome[] = [];
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
      ...(input.runAttempt !== undefined && {
        run_attempt: `${input.runAttempt}`,
      }),
    },
    ...(partialStore !== undefined && {
      onOutcome: (outcome: SampleOutcome) => {
        collectedOutcomes.push(outcome);
      },
    }),
    ...(priorOutcomes.length > 0 && {
      skipSampleEpochs: new Set(
        priorOutcomes.map((outcome) =>
          sampleEpochKey(
            outcome.sampleScore.sampleId,
            outcome.sampleScore.epoch
          )
        )
      ),
    }),
  };
  const fullBenchmarkLayer = benchmarkLayer.pipe(
    layerProvide(FetchHttpClient.layer)
  );
  const resolverLayer = layerSucceed(
    GenerationResolver,
    makeOpenRouterGenerationResolver({
      apiKey: input.apiKey,
      ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
    })
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
  let harnessResult: RunResult;
  try {
    harnessResult = await runHarnessPromise(
      input.runAttempt === undefined
        ? program
        : withRunAttempt(input.runAttempt, program),
      runOpts
    );
  } catch (error) {
    await flushPartialOutcomes({
      partialStore,
      abortSignal: input.abortSignal,
      runScope,
      outcomes: [...priorOutcomes, ...collectedOutcomes],
      newOutcomeCount: collectedOutcomes.length,
    });
    return Either.left(String(error));
  }
  const result =
    priorOutcomes.length > 0
      ? aggregateOutcomes([...priorOutcomes, ...collectedOutcomes])
      : harnessResult;
  if (input.resultStore === undefined) {
    await removePartialOutcomes(partialStore);
    return Either.right({ result, resultsPath: null });
  }
  try {
    const resultsPath = await runHarnessPromise(
      input.resultStore.write({
        result,
        benchmark,
        benchmarkConfig: input.benchmarkConfig,
        epochs: input.epochs,
        sessionId: input.sessionId,
      })
    );
    await removePartialOutcomes(partialStore);
    return Either.right({ result, resultsPath });
  } catch (storeErr) {
    wLog("Failed to persist benchmark results", {
      error: String(storeErr),
    });
    return Either.right({ result, resultsPath: null });
  }
}

async function readPartialOutcomes(
  partialStore: PartialOutcomeStoreService,
  runScope: PartialOutcomeRunScope
): Promise<readonly SampleOutcome[]> {
  let payload: PartialOutcomesPayload | null;
  try {
    payload = await partialStore.read();
  } catch (error) {
    wLog("Failed to read partial benchmark outcomes; starting fresh", {
      error: String(error),
    });
    return [];
  }
  if (payload === null) {
    return [];
  }
  if (!isSameRunScope(payload.scope, runScope)) {
    wLog("Discarding partial benchmark outcomes from a mismatched run scope", {
      persisted_epochs: payload.scope.epochs,
      current_epochs: runScope.epochs,
      persisted_range: JSON.stringify(payload.scope.range ?? null),
      current_range: JSON.stringify(runScope.range ?? null),
      discarded_outcome_count: payload.outcomes.length,
    });
    return [];
  }
  return payload.outcomes;
}

async function removePartialOutcomes(
  partialStore: PartialOutcomeStoreService | undefined
): Promise<void> {
  if (partialStore === undefined) {
    return;
  }
  await partialStore.remove().catch(() => {});
}

async function flushPartialOutcomes(opts: {
  readonly partialStore: PartialOutcomeStoreService | undefined;
  readonly abortSignal: AbortSignal | undefined;
  readonly runScope: PartialOutcomeRunScope;
  readonly outcomes: readonly SampleOutcome[];
  readonly newOutcomeCount: number;
}): Promise<void> {
  const { partialStore, abortSignal, runScope, outcomes, newOutcomeCount } =
    opts;
  if (
    partialStore === undefined ||
    abortSignal?.aborted !== true ||
    newOutcomeCount === 0
  ) {
    return;
  }
  try {
    await partialStore.write({ scope: runScope, outcomes });
    wLog("Flushed partial benchmark outcomes after abort", {
      outcome_count: outcomes.length,
      new_outcome_count: newOutcomeCount,
    });
  } catch (error) {
    wLog("Failed to flush partial benchmark outcomes after abort", {
      error: String(error),
    });
  }
}

export function datasetSizeById(
  benchmarkId: string,
  hostBenchmark?: Benchmark<HostBenchmarkRunConfig>
): AsyncEither<number, string> {
  const benchmarkResult = resolveBenchmark(benchmarkId, hostBenchmark);
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
  hostBenchmark: Benchmark<HostBenchmarkRunConfig> | undefined
): Either.Either<BenchmarkMetadata, string> {
  if (hostBenchmark !== undefined) {
    return hostBenchmark.id === benchmarkId
      ? Either.right(hostBenchmark)
      : Either.left(
          `Benchmark id mismatch: requested "${benchmarkId}", supplied "${hostBenchmark.id}"`
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
    if (input.hostBenchmark !== undefined) {
      return Either.left(
        `A host benchmark cannot be supplied for native benchmark "${input.benchmarkId}"`
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
  if (input.hostBenchmark === undefined) {
    return Either.left(
      `A host benchmark is required for host config "${input.benchmarkId}"`
    );
  }
  if (input.hostBenchmark.id !== input.benchmarkId) {
    return Either.left(
      `Benchmark id mismatch: requested "${input.benchmarkId}", supplied "${input.hostBenchmark.id}"`
    );
  }
  return Either.right({
    benchmark: input.hostBenchmark,
    benchmarkLayer: makeBenchmarkLayer(
      input.hostBenchmark,
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
  const benchmarkInput: BenchmarkRunInput<Config> = {
    apiKey: input.apiKey,
    benchmarkConfig,
    ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
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
