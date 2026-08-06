import type { Effect } from "effect/Effect";
import { fail, flatMap, gen, succeed } from "effect/Effect";

import type { Score, Target, TaskState } from "../../../harness/core";
import { ScoreValue, SolverError } from "../../../harness/core";
import type { RunResult } from "../../../harness/run";
import type { SolverService } from "../../../harness/solver";
import { Either } from "../../../internal/either";
import { parseSchema, z } from "../../../internal/zod";
import type { JudgeConfig } from "../../../judge/judge";
import { judgeCall } from "../../../judge/judge";
import type { ResponsesService } from "../../../providers/responses-client";
import { DSQA_META } from "../../benchmark-meta";
import type { Benchmark, BenchmarkPrimaryScore } from "../../types";
import { makeSearchBenchmarkLayer } from "../core/benchmark";
import { DEEP_RESEARCH_INSTRUCTIONS } from "../core/prompts";
import type { SearchSolverOptions } from "../core/solver";
import { searchSolver } from "../core/solver";
import { mergeModelUsages } from "../core/usage";
import { makeDsqaDatasetLayer } from "./dataset";
import type { DsqaVerdict } from "./grader";
import { DSQA_JUDGE_CONFIG, DsqaVerdictSchema, dsqaJudgeSpec } from "./grader";

export const DSQA_BENCHMARK_ID = DSQA_META.id;

const VERDICT_METADATA_KEY = "verdict" as const;

export const DSQA_METRIC_NAMES = [
  "precision",
  "recall",
  "f1_score",
  "fully_correct",
  "fully_incorrect",
  "partially_correct",
  "correct_with_extraneous_answers",
] as const;

const DsqaMetricsSchema = z.object({
  precision: z.number().min(0).max(1),
  recall: z.number().min(0).max(1),
  f1_score: z.number().min(0).max(1),
  fully_correct: z.number().min(0).max(1),
  fully_incorrect: z.number().min(0).max(1),
  partially_correct: z.number().min(0).max(1),
  correct_with_extraneous_answers: z.number().min(0).max(1),
});

type DsqaMetrics = z.infer<typeof DsqaMetricsSchema>;

const ZERO_DSQA_METRICS: DsqaMetrics = {
  precision: 0,
  recall: 0,
  f1_score: 0,
  fully_correct: 0,
  fully_incorrect: 1,
  partially_correct: 0,
  correct_with_extraneous_answers: 0,
};

const DsqaTrajectoryGradeSchema = z.object({
  kind: z.literal("dsqa_grade"),
  verdict: DsqaVerdictSchema,
  metrics: DsqaMetricsSchema,
});

type DsqaTrajectoryGrade = z.infer<typeof DsqaTrajectoryGradeSchema>;

export function calculateDsqaGrade(
  verdict: DsqaVerdict
): DsqaTrajectoryGrade {
  const details = Object.values(verdict.correctness_details);
  const truePositives = details.filter(Boolean).length;
  const falseNegatives = details.length - truePositives;
  const falsePositives = verdict.excessive_answers.length;
  const precisionDenominator = truePositives + falsePositives;
  const recallDenominator = truePositives + falseNegatives;
  const precision =
    precisionDenominator === 0 ? 0 : truePositives / precisionDenominator;
  const recall =
    recallDenominator === 0 ? 0 : truePositives / recallDenominator;
  const f1Score =
    precision + recall === 0
      ? 0
      : (2 * precision * recall) / (precision + recall);
  const allExpectedAnswersFound = details.every(Boolean);
  const fullyCorrect =
    details.length > 0 &&
    allExpectedAnswersFound &&
    verdict.excessive_answers.length === 0;
  const fullyIncorrect = truePositives === 0;
  const correctWithExtraneousAnswers =
    details.length > 0 &&
    allExpectedAnswersFound &&
    verdict.excessive_answers.length > 0;
  return {
    kind: "dsqa_grade",
    verdict,
    metrics: {
      precision,
      recall,
      f1_score: f1Score,
      fully_correct: fullyCorrect ? 1 : 0,
      fully_incorrect: fullyIncorrect ? 1 : 0,
      partially_correct:
        fullyCorrect || fullyIncorrect || correctWithExtraneousAnswers ? 0 : 1,
      correct_with_extraneous_answers: correctWithExtraneousAnswers ? 1 : 0,
    },
  };
}

function judgeStage(
  responses: ResponsesService,
  judgeConfig: JudgeConfig
): SolverService {
  return (state) =>
    gen(function* () {
      const answer = state.output?.completion ?? "";
      if (answer === "") {
        return state;
      }
      const promptType = state.sample.metadata?.["prompt_type"];
      if (typeof promptType !== "string") {
        return yield* fail(
          new SolverError({ message: "DSQA sample is missing prompt_type" })
        );
      }
      const judged = yield* judgeCall(
        responses,
        judgeConfig,
        dsqaJudgeSpec({
          question: state.sample.input,
          promptType,
          correctAnswer: state.sample.target.text,
          response: answer,
        })
      );
      const usage = mergeModelUsages([state.output?.usage, judged.usage]);
      return {
        ...state,
        ...(state.output !== undefined && {
          output: { ...state.output, ...(usage !== undefined && { usage }) },
        }),
        sample: {
          ...state.sample,
          metadata: {
            ...state.sample.metadata,
            [VERDICT_METADATA_KEY]: judged.verdict,
          },
        },
      };
    });
}

export function makeDsqaSolver(
  responses: ResponsesService,
  options: SearchSolverOptions
): SolverService {
  const solve = searchSolver(responses, options);
  const judge = judgeStage(responses, {
    ...DSQA_JUDGE_CONFIG,
    ...(options.versionOverride !== undefined && {
      versionOverride: options.versionOverride,
    }),
    ...(options.retry !== undefined && { retry: options.retry }),
  });
  return (state) => solve(state).pipe(flatMap(judge));
}

export function dsqaScorer(
  state: TaskState,
  _target: Target
): Effect<Score, never> {
  const raw = state.sample.metadata?.[VERDICT_METADATA_KEY];
  const parsed = parseSchema(DsqaVerdictSchema, raw);
  if (Either.isLeft(parsed)) {
    return succeed({
      value: ScoreValue.Incorrect,
      answer: state.output?.completion ?? null,
      explanation:
        raw === undefined ? "no verdict" : "verdict failed validation",
    });
  }
  const verdict: DsqaVerdict = parsed.right;
  const grade = calculateDsqaGrade(verdict);
  return succeed({
    value:
      grade.metrics.fully_correct === 1
        ? ScoreValue.Correct
        : ScoreValue.Incorrect,
    answer: state.output?.completion ?? null,
    explanation: verdict.explanation,
    trajectory: { kind: "judge_runs", runs: [grade] } as const,
  });
}

function dsqaQuestionMetrics(result: RunResult): readonly DsqaMetrics[] {
  const metricsBySample = new Map<string, DsqaMetrics[]>();
  for (const sample of result.sampleScores) {
    if (sample.score.value === ScoreValue.Skipped) {
      continue;
    }
    const trajectory = sample.score.trajectory;
    let metrics = ZERO_DSQA_METRICS;
    if (trajectory?.kind === "judge_runs") {
      const parsed = parseSchema(DsqaTrajectoryGradeSchema, trajectory.runs[0]);
      if (Either.isRight(parsed)) {
        metrics = parsed.right.metrics;
      }
    }
    const existing = metricsBySample.get(sample.sampleId);
    if (existing) {
      existing.push(metrics);
    } else {
      metricsBySample.set(sample.sampleId, [metrics]);
    }
  }
  return [...metricsBySample.values()].map((metrics) =>
    Object.fromEntries(
      DSQA_METRIC_NAMES.map((name) => [
        name,
        metrics.reduce((sum, item) => sum + item[name], 0) / metrics.length,
      ])
    ) as DsqaMetrics
  );
}

export function dsqaRunLevelScores(result: RunResult): readonly {
  name: string;
  metrics: Readonly<Record<string, { value: number }>>;
}[] {
  const metrics = dsqaQuestionMetrics(result);
  if (metrics.length === 0) {
    return [];
  }
  return [
    {
      name: "dsqa",
      metrics: {
        ...Object.fromEntries(
          DSQA_METRIC_NAMES.map((name) => [
            name,
            {
              value:
                metrics.reduce((sum, item) => sum + item[name], 0) /
                metrics.length,
            },
          ])
        ),
        samples_judged: { value: metrics.length },
      },
    },
  ];
}

export function dsqaPrimaryScore(
  result: RunResult
): BenchmarkPrimaryScore | undefined {
  const metrics = dsqaQuestionMetrics(result);
  if (metrics.length === 0) {
    return undefined;
  }
  return {
    value:
      metrics.reduce((sum, item) => sum + item.f1_score, 0) / metrics.length,
    weight: metrics.length,
  };
}

export const DSQA_BENCHMARK: Benchmark = {
  id: DSQA_BENCHMARK_ID,
  makeDatasetLayer: makeDsqaDatasetLayer,
  temperature: 0,
  defaultEpochs: DSQA_META.defaultEpochs,
  makeLayer: (input) =>
    makeSearchBenchmarkLayer(input, {
      benchmarkId: DSQA_BENCHMARK_ID,
      instructions: DEEP_RESEARCH_INSTRUCTIONS,
      temperature: 0,
      makeDatasetLayer: makeDsqaDatasetLayer,
      makeSolver: makeDsqaSolver,
      scorer: dsqaScorer,
    }),
  runLevelScores: dsqaRunLevelScores,
  primaryScore: dsqaPrimaryScore,
};
