import type { Effect } from "effect/Effect";
import { flatMap, gen, succeed } from "effect/Effect";

import type { Score, Target, TaskState } from "../../../harness/core";
import { ScoreValue } from "../../../harness/core";
import type { RunResult } from "../../../harness/run";
import type { SolverService } from "../../../harness/solver";
import { Either } from "../../../internal/either";
import { definedValues } from "../../../internal/guards";
import { parseSchema } from "../../../internal/zod";
import type { JudgeConfig } from "../../../judge/judge";
import { judgeCall } from "../../../judge/judge";
import type { ResponsesService } from "../../../providers/responses-client";
import type { SearchSolverOptions } from "../core/solver";
import { searchSolver } from "../core/solver";
import { mergeModelUsages } from "../core/usage";
import type { AnswerEquivalenceVerdict } from "./answer-equivalence";
import {
  ANSWER_EQUIVALENCE_JUDGE_CONFIG,
  AnswerEquivalenceVerdictSchema,
  answerEquivalenceJudgeSpec,
} from "./answer-equivalence";

const VERDICT_METADATA_KEY = "verdict" as const;

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
      const judged = yield* judgeCall(
        responses,
        judgeConfig,
        answerEquivalenceJudgeSpec({
          question: state.sample.input,
          response: answer,
          correctAnswer: state.sample.target.text,
        })
      );
      const usage = mergeModelUsages([state.output?.usage, judged.usage]);
      return {
        ...state,
        ...definedValues({
          output:
            state.output !== undefined
              ? {
                  ...state.output,
                  ...definedValues({
                    usage,
                  }),
                }
              : undefined,
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

export function makeAnswerEquivalenceSolver(
  responses: ResponsesService,
  options: SearchSolverOptions
): SolverService {
  const solve = searchSolver(responses, options);
  const judge = judgeStage(responses, {
    ...ANSWER_EQUIVALENCE_JUDGE_CONFIG,
    ...definedValues({
      versionOverride: options.versionOverride,
      retry: options.retry,
    }),
  });
  return (state) => solve(state).pipe(flatMap(judge));
}

export function answerEquivalenceScorer(
  state: TaskState,
  _target: Target
): Effect<Score, never> {
  const raw = state.sample.metadata?.[VERDICT_METADATA_KEY];
  const parsed = parseSchema(AnswerEquivalenceVerdictSchema, raw);
  if (Either.isLeft(parsed)) {
    return succeed({
      value: ScoreValue.Incorrect,
      answer: state.output?.completion ?? null,
      explanation:
        raw === undefined
          ? "no verdict (empty answer or judge unavailable)"
          : "verdict failed validation",
    });
  }
  const verdict: AnswerEquivalenceVerdict = parsed.right;
  return succeed({
    value:
      verdict.correct === "yes" ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: verdict.extracted_final_answer,
    explanation: verdict.reasoning,
    trajectory: { kind: "judge_runs", runs: [verdict] } as const,
  });
}

export function answerEquivalenceRunLevelScores(
  result: RunResult,
  name: string
): readonly {
  name: string;
  metrics: Readonly<
    Record<
      string,
      {
        value: number;
      }
    >
  >;
}[] {
  const verdicts = result.sampleScores
    .map((sampleScore) => sampleScore.score.trajectory)
    .filter(
      (trajectory) =>
        trajectory !== undefined && trajectory.kind === "judge_runs"
    )
    .map((trajectory) =>
      parseSchema(AnswerEquivalenceVerdictSchema, trajectory.runs[0])
    )
    .filter(Either.isRight)
    .map((parsed) => parsed.right);
  if (verdicts.length === 0) {
    return [];
  }
  const confidenceSum = verdicts.reduce(
    (sum, verdict) => sum + verdict.confidence,
    0
  );
  return [
    {
      name,
      metrics: {
        mean_stated_confidence: { value: confidenceSum / verdicts.length },
        samples_judged: { value: verdicts.length },
      },
    },
  ];
}
