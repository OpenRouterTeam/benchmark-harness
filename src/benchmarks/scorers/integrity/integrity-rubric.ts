import type { Effect } from "effect/Effect";
import { catch as catch_, flatMap, map, succeed } from "effect/Effect";
import * as Result from "effect/Result";

import type { Score, TaskState } from "../../../harness/core";
import { MessageRole } from "../../../harness/core";
import type { RunResult } from "../../../harness/run";
import type { ScorerService } from "../../../harness/scorer";
import { parseSchema, z } from "../../../internal/zod";
import type {
  DecisionChoiceQuestion,
  DecisionsError,
} from "../../../judge/decisions";
import {
  deterministicIntegrity,
  INTEGRITY_QUESTION,
  INTEGRITY_SCORE_NAME,
  judgedIntegrity,
} from "./integrity-spec";

export type ChoiceJudge = (
  state: Readonly<Record<string, string>>,
  question: DecisionChoiceQuestion
) => Effect<Readonly<Record<string, number>>, DecisionsError>;

export {
  INTEGRITY_FLAG_THRESHOLD,
  INTEGRITY_JUDGE_MODEL,
  INTEGRITY_QUESTION,
  INTEGRITY_SCORE_NAME,
} from "./integrity-spec";

const IntegrityVerdictSchema = z.discriminatedUnion("source", [
  z.object({
    judge: z.literal(INTEGRITY_SCORE_NAME),
    source: z.enum(["judge", "empty", "bare_answer"]),
    notIntact: z.number(),
    flagged: z.boolean(),
    probabilities: z.record(z.string(), z.number()).optional(),
  }),
  z.object({
    judge: z.literal(INTEGRITY_SCORE_NAME),
    source: z.literal("error"),
    error: z.string(),
  }),
]);

export type IntegrityVerdict = z.infer<typeof IntegrityVerdictSchema>;

export function withIntegrityRubric(
  base: ScorerService,
  judge: ChoiceJudge
): ScorerService {
  return (state, target) =>
    base(state, target).pipe(
      flatMap((score) => {
        const completion = state.output?.completion ?? "";
        const deterministic = deterministicIntegrity(completion);
        if (deterministic !== undefined) {
          return succeed(
            withVerdict(score, {
              judge: INTEGRITY_SCORE_NAME,
              ...deterministic,
            })
          );
        }
        return judge(judgeState(state, completion), INTEGRITY_QUESTION).pipe(
          map((probabilities) => withVerdict(score, judged(probabilities))),
          catch_((error) => succeed(withVerdict(score, failed(error.message))))
        );
      })
    );
}

export function integrityRunLevelScores(result: RunResult): readonly {
  readonly name: string;
  readonly metrics: Readonly<Record<string, { readonly value: number }>>;
}[] {
  const verdicts = result.sampleScores.flatMap((sample) => {
    const trajectory = sample.score.trajectory;
    if (trajectory?.kind !== "judge_runs") {
      return [];
    }
    const parsed = parseSchema(IntegrityVerdictSchema, trajectory.runs[0]);
    return Result.isSuccess(parsed) ? [parsed.success] : [];
  });
  if (verdicts.length === 0) {
    return [];
  }
  const scored = verdicts.flatMap((verdict) =>
    verdict.source === "error" ? [] : [verdict]
  );
  const errors = verdicts.length - scored.length;
  const flagged = scored.filter((verdict) => verdict.flagged).length;
  const notIntact = scored.reduce((sum, verdict) => sum + verdict.notIntact, 0);
  const rates: Record<string, { readonly value: number }> =
    scored.length === 0
      ? {}
      : {
          flagged_rate: { value: flagged / scored.length },
          mean_not_intact: { value: notIntact / scored.length },
        };
  return [
    {
      name: INTEGRITY_SCORE_NAME,
      metrics: {
        ...rates,
        flagged: { value: flagged },
        scored: { value: scored.length },
        errors: { value: errors },
      },
    },
  ];
}

function judgeState(
  state: TaskState,
  completion: string
): Record<string, string> {
  const userRequest =
    state.messages.findLast((message) => message.role === MessageRole.User)
      ?.content ?? "";
  return { user_request: userRequest, completion };
}

function judged(
  probabilities: Readonly<Record<string, number>>
): IntegrityVerdict {
  const assessment = judgedIntegrity(probabilities);
  if (assessment === undefined) {
    return failed("judge returned no coherent_good probability");
  }
  return {
    judge: INTEGRITY_SCORE_NAME,
    ...assessment,
    probabilities: { ...probabilities },
  };
}

function failed(error: string): IntegrityVerdict {
  return { judge: INTEGRITY_SCORE_NAME, source: "error", error };
}

function withVerdict(score: Score, verdict: IntegrityVerdict): Score {
  return { ...score, trajectory: { kind: "judge_runs", runs: [verdict] } };
}
