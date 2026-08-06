import type { Effect } from "effect/Effect";
import {
  catchTags,
  fail as effectFail,
  flatMap as effectFlatMap,
  gen as effectGen,
  map as effectMap,
  annotateLogs,
  withLogSpan,
  succeed as effectSucceed,
} from "effect/Effect";
import type { Stream } from "effect/Stream";
import {
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

interface FoldAccumulator {
  scores: SampleScore[];
  usage: UsageTotals;
}

type EvalOutcome = {
  sampleScore: SampleScore;
  usage?: ModelUsage;
  generationTimeMs?: number;
};

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
  evaluate: Effect<
    EvalOutcome,
    ModelError | SolverError,
    Solver | Scorer | ProgressReporter | CheckpointStore
  >
): Effect<
  EvalOutcome,
  ModelError | SolverError,
  Solver | Scorer | ProgressReporter | CheckpointStore
> {
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
): Effect<
  EvalOutcome,
  ModelError | SolverError,
  Solver | Scorer | ProgressReporter | CheckpointStore
> {
  const { sampleEpoch } = opts;
  const { sample, epoch } = sampleEpoch;
  const evaluation = effectGen(function* () {
    const solver = yield* Solver;
    const scorer = yield* Scorer;
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
        ...(state.requestBody !== undefined && {
          requestBody: state.requestBody,
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
      SolverError: (solverErr) =>
        opts.degradeSolverErrors
          ? effectSucceed(
              errorOutcome({
                sample,
                epoch,
                value: ScoreValue.Incorrect,
                explanation: `Solver error: ${solverErr.message}`,
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

export function runBenchmark(
  config: RunConfig
): Effect<
  RunResult,
  ModelError | SolverError | DatasetError,
  Dataset | Solver | Scorer | ProgressReporter | CheckpointStore
> {
  return Dataset.pipe(
    effectFlatMap((dataset) => {
      const sampleEpochs = sampleEpochStream(
        dataset,
        config.epochs,
        config.range
      );
      const initialAcc: FoldAccumulator = {
        scores: [],
        usage: { ...ZERO_USAGE },
      };
      return sampleEpochs.pipe(
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
            ),
          { concurrency: config.maxConcurrency }
        ),
        streamRunFoldEffect(initialAcc, (acc, item) =>
          effectGen(function* () {
            const updated = accumulateOutcome(acc, item);
            const reporter = yield* ProgressReporter;
            yield* reporter.onSampleComplete(updated.scores.length);
            return updated;
          })
        ),
        effectMap(finalizeRun)
      );
    }),
    annotateLogs(config.logAnnotations ?? {})
  );
}
