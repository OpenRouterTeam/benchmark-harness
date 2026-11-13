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

export type ChoiceJudge = (
  state: Readonly<Record<string, string>>,
  question: DecisionChoiceQuestion
) => Effect<Readonly<Record<string, number>>, DecisionsError>;

export const INTEGRITY_JUDGE_MODEL = "openai/gpt-6-luna-decisions";

export const INTEGRITY_FLAG_THRESHOLD = 0.4;

export const INTEGRITY_SCORE_NAME = "integrity";

export const INTEGRITY_QUESTION = {
  type: "choice",
  instructions:
    'Judge only whether `completion` was emitted intact. Look closely at the words themselves: inference bugs show up as dropped tokens (missing words that leave sentences ungrammatical), duplicated tokens ("the the", "oror"), fragments of two words fused together, truncated words, mojibake or replacement characters, and loops of repeated text.',
  criteria: {
    coherent_good:
      "Intact. Every sentence is grammatical and every word is whole; nothing is missing, doubled, or fused, apart from at most one ordinary typo.",
    coherent_okay:
      "A few token-level glitches (a doubled or missing word, a fused fragment) but otherwise clean.",
    coherent_bad:
      "Visibly corrupted: repeated token-level glitches such as missing or doubled words, fused word fragments, garbled characters, or a repetition loop; or empty output.",
  },
} as const satisfies DecisionChoiceQuestion;

const BARE_ANSWER = /^\s*Answer:\s*[A-J]\s*$/i;

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
        if (completion.trim() === "") {
          return succeed(withVerdict(score, deterministic("empty", 1)));
        }
        if (BARE_ANSWER.test(completion)) {
          return succeed(withVerdict(score, deterministic("bare_answer", 0)));
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

function deterministic(
  source: "empty" | "bare_answer",
  notIntact: number
): IntegrityVerdict {
  return {
    judge: INTEGRITY_SCORE_NAME,
    source,
    notIntact,
    flagged: notIntact > INTEGRITY_FLAG_THRESHOLD,
  };
}

function judged(
  probabilities: Readonly<Record<string, number>>
): IntegrityVerdict {
  const good = probabilities["coherent_good"];
  if (good === undefined) {
    return failed("judge returned no coherent_good probability");
  }
  const notIntact = 1 - good;
  return {
    judge: INTEGRITY_SCORE_NAME,
    source: "judge",
    notIntact,
    flagged: notIntact > INTEGRITY_FLAG_THRESHOLD,
    probabilities: { ...probabilities },
  };
}

function failed(error: string): IntegrityVerdict {
  return { judge: INTEGRITY_SCORE_NAME, source: "error", error };
}

function withVerdict(score: Score, verdict: IntegrityVerdict): Score {
  return { ...score, trajectory: { kind: "judge_runs", runs: [verdict] } };
}
