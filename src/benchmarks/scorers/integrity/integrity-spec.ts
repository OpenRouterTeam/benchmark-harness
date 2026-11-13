import type { DecisionChoiceQuestion } from "../../../judge/decisions";

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

export type IntegritySource = "judge" | "empty" | "bare_answer";

export interface IntegrityAssessment {
  readonly source: IntegritySource;
  readonly notIntact: number;
  readonly flagged: boolean;
}

export function deterministicIntegrity(
  completion: string
): IntegrityAssessment | undefined {
  if (completion.trim() === "") {
    return assessment("empty", 1);
  }
  if (BARE_ANSWER.test(completion)) {
    return assessment("bare_answer", 0);
  }
  return undefined;
}

export function judgedIntegrity(
  probabilities: Readonly<Record<string, number>>
): IntegrityAssessment | undefined {
  const good = probabilities["coherent_good"];
  return good === undefined ? undefined : assessment("judge", 1 - good);
}

function assessment(
  source: IntegritySource,
  notIntact: number
): IntegrityAssessment {
  return { source, notIntact, flagged: notIntact > INTEGRITY_FLAG_THRESHOLD };
}
