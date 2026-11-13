import type { Effect } from "effect/Effect";
import { catch as catch_, flatMap, map, succeed } from "effect/Effect";

import type { Score, TaskState } from "../../../harness/core";
import { MessageRole, ScoreValue } from "../../../harness/core";
import type { ScorerService } from "../../../harness/scorer";
import type { ClefChoiceQuestion, ClefError } from "../../../judge/clef";

export type ChoiceJudge = (
  state: Readonly<Record<string, string>>,
  question: ClefChoiceQuestion
) => Effect<Readonly<Record<string, number>>, ClefError>;

export const INTEGRITY_FLAG_THRESHOLD = 0.58;

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
} as const satisfies ClefChoiceQuestion;

const BARE_ANSWER = /^\s*Answer:\s*[A-J]\s*$/i;

export function withIntegrityRubric(
  base: ScorerService,
  judge: ChoiceJudge
): ScorerService {
  return (state, target) =>
    base(state, target).pipe(
      flatMap((score) => {
        const completion = state.output?.completion ?? "";
        if (completion.trim() === "" || BARE_ANSWER.test(completion)) {
          return succeed(score);
        }
        return judge(judgeState(state, completion), INTEGRITY_QUESTION).pipe(
          map((probabilities) => applyVerdict(score, probabilities)),
          catch_((error) =>
            succeed({
              ...score,
              explanation: `${score.explanation}; integrity check skipped: ${error.message}`,
            })
          )
        );
      })
    );
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

function applyVerdict(
  score: Score,
  probabilities: Readonly<Record<string, number>>
): Score {
  const notIntact = 1 - (probabilities["coherent_good"] ?? 0);
  const trajectory = {
    kind: "judge_runs",
    runs: [{ judge: "clef_integrity", probabilities, notIntact }],
  } as const;
  if (notIntact <= INTEGRITY_FLAG_THRESHOLD) {
    return { ...score, trajectory };
  }
  return {
    ...score,
    value:
      score.value === ScoreValue.Skipped ? score.value : ScoreValue.Incorrect,
    explanation: `${score.explanation}; completion flagged as corrupted (P(not intact)=${notIntact.toFixed(3)})`,
    trajectory,
  };
}
