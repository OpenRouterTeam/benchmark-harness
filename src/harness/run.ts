import type { Effect } from "effect/Effect";
import {
  catchAll as effectCatchAll,
  catchTags,
  fail as effectFail,
  flatMap as effectFlatMap,
  gen as effectGen,
  logWarning,
  map as effectMap,
  annotateLogs,
  retry as effectRetry,
  tap as effectTap,
  tapError as effectTapError,
  tryPromise as effectTryPromise,
  withLogSpan,
  succeed as effectSucceed,
} from "effect/Effect";
import { exponential, intersect, jittered, recurs } from "effect/Schedule";
import type { Stream } from "effect/Stream";
import {
  filter as streamFilter,
  flatMap as streamFlatMap,
  fromIterable as streamFromIterable,
  mapEffect as streamMapEffect,
  runFoldEffect as streamRunFoldEffect,
  zipWithIndex as streamZipWithIndex,
} from "effect/Stream";

import {
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../runtime/generation-ids";
import type {
  DatasetError,
  ModelError,
  ModelUsage,
  Sample,
  Score,
  SolverError,
  UsageTotals,
} from "./core";
import {
  initialTaskState,
  isRetryableModelError,
  isSystemicModelError,
  ScoreValue,
} from "./core";
import type { DatasetService } from "./dataset";
import { Dataset } from "./dataset";
import type { AggregateMetrics, SampleScore } from "./metric";
import { aggregateScores } from "./metric";
import { CheckpointStore, ProgressReporter } from "./progress";
import { setBenchRequestContext } from "./request-context";
import type { CompletedSampleEntry } from "./sample-result-store";
import {
  SampleResultStore,
  SampleResultStoreError,
} from "./sample-result-store";
import { Scorer } from "./scorer";
import { Solver } from "./solver";

export interface RunConfig {
  readonly epochs: number;
  readonly maxConcurrency: number;
  readonly range?: {
    readonly start?: number;
    readonly end?: number;
  };
  readonly degradeSolverErrors?: boolean;
  readonly logAnnotations?: Readonly<Record<string, string>>;
}

export interface RunResult {
  readonly metrics: AggregateMetrics;
  readonly usage: UsageTotals;
  readonly sampleScores: readonly SampleScore[];
}

interface SampleEpoch {
  readonly sample: Sample;
  readonly epoch: number;
  readonly sampleIndex: number;
}

/** Services every per-sample evaluation reaches for. */
type EvalContext = Solver | Scorer | ProgressReporter | CheckpointStore;

interface FoldAccumulator {
  scores: SampleScore[];
  usage: UsageTotals;
}

type EvalOutcome = {
  sampleScore: SampleScore;
  usage?: ModelUsage;
  generationTimeMs?: number;
  /**
   * The score was synthesized from a model/solver failure rather than an actual
   * evaluation. Such outcomes are persisted only at the end of the run, marked
   * `degraded` in the record, and ignored by the retry skip-list — so a later
   * activity attempt re-runs the sample instead of freezing a capacity failure
   * into the results, while the finalization fold can still count them.
   */
  degraded?: true;
};

/** Persist the run's degraded outcomes, deferred to after every sample has settled. */
function persistDegradedOutcomes(
  outcomes: Iterable<readonly [SampleEpoch, EvalOutcome]>
): Effect<void, SampleResultStoreError, SampleResultStore> {
  return effectGen(function* () {
    for (const [sampleEpoch, outcome] of outcomes) {
      yield* writeEntry(sampleEpoch, outcome);
    }
  });
}

function sampleEpochStream(
  dataset: DatasetService,
  epochs: number,
  range:
    | {
        readonly start?: number;
        readonly end?: number;
      }
    | undefined
): Stream<SampleEpoch, DatasetError> {
  const baseIndex = range?.start ?? 0;
  return dataset.stream(range).pipe(
    streamZipWithIndex,
    streamFlatMap(([sample, i]) =>
      streamFromIterable(
        Array.from({ length: epochs }, (_, epoch) => ({
          sample,
          epoch,
          sampleIndex: baseIndex + i,
        }))
      )
    )
  );
}

function evalWithProgress(
  sampleEpoch: SampleEpoch,
  evaluate: Effect<EvalOutcome, ModelError | SolverError, EvalContext>
): Effect<EvalOutcome, ModelError | SolverError, EvalContext> {
  const { sample, epoch, sampleIndex } = sampleEpoch;
  return effectGen(function* () {
    const reporter = yield* ProgressReporter;
    yield* reporter.onSampleStart({
      type: "sample-start",
      sampleIndex,
      sampleId: sample.id,
      epoch,
    });
    try {
      return yield* evaluate;
    } finally {
      yield* reporter.onSampleEnd({
        type: "sample-end",
        sampleId: sample.id,
        epoch,
      });
    }
  });
}

function sampleEpochKey(sampleIndex: number, epoch: number): string {
  return `${sampleIndex}:${epoch}`;
}

/**
 * Completed (sample, epoch) records already durable for this range, keyed by
 * `sampleIndex:epoch`. Degraded records are ignored — a retry re-runs those
 * samples instead of freezing an infrastructure failure into the results. A
 * store failure is never fatal: the run falls back to re-evaluating
 * everything.
 */
function loadCompletedEntries(
  config: RunConfig
): Effect<ReadonlyMap<string, CompletedSampleEntry>, never, SampleResultStore> {
  return effectGen(function* () {
    const store = yield* SampleResultStore;
    const noEntries: readonly CompletedSampleEntry[] = [];
    const entries = yield* effectTryPromise({
      try: () => store.list(config.range ?? {}),
      catch: (e: unknown) =>
        new SampleResultStoreError({
          message: `Failed to list sample results: ${String(e)}`,
        }),
    }).pipe(
      effectCatchAll((error) =>
        logWarning("Failed to list existing sample results", {
          error: error.message,
        }).pipe(effectMap(() => noEntries))
      )
    );
    const inRange = entries
      .filter(
        (entry) =>
          entry.degraded !== true &&
          entry.epoch >= 0 &&
          entry.epoch < config.epochs
      )
      .toSorted((a, b) => a.sampleIndex - b.sampleIndex || a.epoch - b.epoch);
    return new Map(
      inRange.map((entry) => [
        sampleEpochKey(entry.sampleIndex, entry.epoch),
        entry,
      ])
    );
  });
}

/** Fold the already-completed records in so the final aggregate matches an uninterrupted run. */
function seedAccumulator(
  entries: Iterable<CompletedSampleEntry>
): FoldAccumulator {
  const acc: FoldAccumulator = { scores: [], usage: { ...ZERO_USAGE } };
  for (const entry of entries) {
    accumulateOutcome(acc, {
      sampleScore: entry.sampleScore,
      ...(entry.usage !== undefined && { usage: entry.usage }),
      ...(entry.generationTimeMs !== undefined && {
        generationTimeMs: entry.generationTimeMs,
      }),
    });
  }
  return acc;
}

/** Max additional write attempts before a record write is declared failed. */
const PERSIST_MAX_RETRIES = 3;
const PERSIST_BASE_DELAY = "200 millis";

/**
 * Write one (sample, epoch) record with bounded retries. The store is the
 * source of truth for completed work, so a write that still fails after the
 * retries fails the run: completing successfully must mean the record is
 * durable, letting the activity retry re-run only what is genuinely missing.
 */
function writeEntry(
  sampleEpoch: SampleEpoch,
  outcome: EvalOutcome
): Effect<void, SampleResultStoreError, SampleResultStore> {
  return effectGen(function* () {
    const store = yield* SampleResultStore;
    yield* effectTryPromise({
      try: () =>
        store.write({
          sampleIndex: sampleEpoch.sampleIndex,
          epoch: sampleEpoch.epoch,
          sampleScore: outcome.sampleScore,
          ...(outcome.usage !== undefined && { usage: outcome.usage }),
          ...(outcome.generationTimeMs !== undefined && {
            generationTimeMs: outcome.generationTimeMs,
          }),
          ...(outcome.degraded === true && { degraded: true }),
        }),
      catch: (e: unknown) =>
        new SampleResultStoreError({
          message: `Failed to persist sample result: ${String(e)}`,
        }),
    }).pipe(
      effectRetry(
        exponential(PERSIST_BASE_DELAY).pipe(
          jittered,
          intersect(recurs(PERSIST_MAX_RETRIES))
        )
      ),
      effectTapError((error) =>
        logWarning("Failed to persist sample result; failing the run", {
          sample_id: outcome.sampleScore.sampleId,
          sample_index: sampleEpoch.sampleIndex,
          epoch: sampleEpoch.epoch,
          degraded: outcome.degraded === true,
          error: error.message,
        })
      )
    );
  });
}

/**
 * Persist one successful result as soon as it completes. Degraded outcomes
 * are deferred to the end of the run (see {@link runBenchmark}) so a mid-run
 * degraded record never shadows a later successful retry of the same
 * (sample, epoch) within this attempt.
 */
function persistOutcome(
  sampleEpoch: SampleEpoch,
  outcome: EvalOutcome
): Effect<void, SampleResultStoreError, SampleResultStore> {
  return outcome.degraded === true
    ? effectSucceed(undefined)
    : writeEntry(sampleEpoch, outcome);
}

function accumulateOutcome(
  acc: FoldAccumulator,
  item: EvalOutcome
): FoldAccumulator {
  acc.scores.push(item.sampleScore);
  const u = item.usage;
  acc.usage = {
    inputTokens: acc.usage.inputTokens + (u?.inputTokens ?? 0),
    outputTokens: acc.usage.outputTokens + (u?.outputTokens ?? 0),
    totalTokens: acc.usage.totalTokens + (u?.totalTokens ?? 0),
    reasoningTokens: acc.usage.reasoningTokens + (u?.reasoningTokens ?? 0),
    totalCost: acc.usage.totalCost + (u?.totalCost ?? 0),
    generationTimeMs: acc.usage.generationTimeMs + (item.generationTimeMs ?? 0),
  };
  return acc;
}

function finalizeRun(acc: FoldAccumulator): RunResult {
  return {
    metrics: aggregateScores(acc.scores),
    usage: acc.usage,
    sampleScores: acc.scores,
  };
}

interface EvaluateOneOpts {
  readonly sampleEpoch: SampleEpoch;
  readonly degradeSolverErrors: boolean;
}

function evaluateOne(
  opts: EvaluateOneOpts
): Effect<EvalOutcome, ModelError | SolverError, EvalContext> {
  const { sampleEpoch } = opts;
  const { sample, epoch } = sampleEpoch;
  const evaluation = effectGen(function* () {
    const solver = yield* Solver;
    const scorer = yield* Scorer;
    /* Stamp the (sample, epoch) identity into this fiber so every model call
       it issues carries it — the bench-gateway keys request coalescing on it,
       otherwise identical bodies across epochs replay one epoch's answer. */
    yield* setBenchRequestContext({ sampleId: String(sample.id), epoch });
    const state = yield* solver(initialTaskState(sample, epoch));
    const score = yield* scorer(state, sample.target);
    return {
      sampleScore: {
        sampleId: sample.id,
        epoch,
        score,
        messages: state.messages,
        ...(state.responseItems !== undefined && {
          responseItems: state.responseItems,
        }),
        ...(state.sample.metadata && { metadata: state.sample.metadata }),
        input: sample.input,
        target: sample.target.text,
      },
      usage: state.output?.usage,
      generationTimeMs: state.output?.generationTimeMs,
    } as const;
  }).pipe(
    catchTags({
      ModelError: (modelErr) => {
        if (isSystemicModelError(modelErr)) {
          return effectFail(modelErr);
        }
        if (isRetryableModelError(modelErr)) {
          return effectSucceed(
            errorOutcome({
              sample,
              epoch,
              value: ScoreValue.Skipped,
              explanation: `Model error (skipped): ${modelErr.message}`,
            })
          );
        }
        return effectSucceed(
          errorOutcome({
            sample,
            epoch,
            value: ScoreValue.Incorrect,
            explanation: `Model error: ${modelErr.message}`,
          })
        );
      },
      /* Degraded solver errors are harness/sandbox infrastructure failures,
         not the model's fault, so they score Skipped — out of the accuracy
         denominator, like exhausted retryable model errors. */
      SolverError: (solverErr) =>
        opts.degradeSolverErrors
          ? effectSucceed(
              errorOutcome({
                sample,
                epoch,
                value: ScoreValue.Skipped,
                explanation: `Solver error (skipped): ${solverErr.message}`,
              })
            )
          : effectFail(solverErr),
    })
  );
  return resetGenerationIds.pipe(
    effectFlatMap(() =>
      evaluation.pipe(
        effectFlatMap((outcome) =>
          getCollectedGenerationIds.pipe(
            effectMap((ids) =>
              ids.length > 0
                ? {
                    ...outcome,
                    sampleScore: {
                      ...outcome.sampleScore,
                      generationIds: [...new Set(ids)],
                    },
                  }
                : outcome
            )
          )
        )
      )
    )
  );
}

interface ErrorOutcomeOpts {
  readonly sample: Sample;
  readonly epoch: number;
  readonly value: ScoreValue;
  readonly explanation: string;
}

function errorOutcome(opts: ErrorOutcomeOpts): EvalOutcome {
  const { sample, epoch, value, explanation } = opts;
  const score: Score = {
    value,
    answer: null,
    explanation,
  };
  return {
    sampleScore: {
      sampleId: sample.id,
      epoch,
      score,
      messages: [],
      ...(sample.metadata && { metadata: sample.metadata }),
      input: sample.input,
      target: sample.target.text,
    },
    degraded: true,
  };
}

const ZERO_USAGE: UsageTotals = {
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  reasoningTokens: 0,
  totalCost: 0,
  generationTimeMs: 0,
};

/**
 * Stream the dataset, fan out across (sample, epoch) pairs with bounded
 * concurrency, solve + score each, and fold into metrics + usage.
 *
 * Non-degraded (sample, epoch) pairs already recorded in the
 * `SampleResultStore` are skipped and their stored results seed the
 * accumulator, so a retried chunk produces the same aggregate as an
 * uninterrupted run. Scores synthesized from a model/solver failure are
 * written only at the end of the run, marked `degraded` — durable for the
 * finalization fold to count, but ignored by the retry skip-list so a retry
 * still re-runs them. Persisting the records is part of completing the run: a
 * write that fails after bounded retries fails the run, so a successful run
 * guarantees every evaluated (sample, epoch) record is durable.
 *
 * `Dataset | Solver | Scorer | ProgressReporter | CheckpointStore |
 * SampleResultStore` are yielded from the environment — provided by the
 * benchmark layer + entry point.
 */
export function runBenchmark(
  config: RunConfig
): Effect<
  RunResult,
  ModelError | SolverError | DatasetError | SampleResultStoreError,
  | Dataset
  | Solver
  | Scorer
  | ProgressReporter
  | CheckpointStore
  | SampleResultStore
> {
  return effectGen(function* () {
    const dataset = yield* Dataset;
    const completed = yield* loadCompletedEntries(config);
    const sampleEpochs = sampleEpochStream(
      dataset,
      config.epochs,
      config.range
    ).pipe(
      streamFilter(
        (se) => !completed.has(sampleEpochKey(se.sampleIndex, se.epoch))
      )
    );
    const initialAcc = seedAccumulator(completed.values());
    const degradedOutcomes: (readonly [SampleEpoch, EvalOutcome])[] = [];

    const folded = yield* sampleEpochs.pipe(
      streamMapEffect(
        (se) =>
          evalWithProgress(
            se,
            evaluateOne({
              sampleEpoch: se,
              degradeSolverErrors: config.degradeSolverErrors ?? false,
            }).pipe(
              annotateLogs({
                sample_id: se.sample.id,
                epoch: se.epoch,
              }),
              withLogSpan("sample")
            )
          ).pipe(
            /* Persist inside the concurrent stage: the downstream fold sees
               elements in input order, so a slow sample would otherwise hold
               every finished result out of durable storage. Degraded outcomes
               are collected instead and written after the stream settles. */
            effectTap((outcome) => {
              if (outcome.degraded === true) {
                degradedOutcomes.push([se, outcome]);
              }
              return persistOutcome(se, outcome);
            })
          ),
        { concurrency: config.maxConcurrency }
      ),
      streamRunFoldEffect(initialAcc, (acc, outcome) =>
        effectGen(function* () {
          const updated = accumulateOutcome(acc, outcome);
          const reporter = yield* ProgressReporter;
          yield* reporter.onSampleComplete(updated.scores.length);
          return updated;
        })
      )
    );

    yield* persistDegradedOutcomes(degradedOutcomes);
    return finalizeRun(folded);
  }).pipe(annotateLogs(config.logAnnotations ?? {}));
}
