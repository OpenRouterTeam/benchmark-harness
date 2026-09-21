import type {
  ChoiceQuestion,
  DecisionsAnswer,
  DecisionsQuestion,
  DecisionsResponse,
  ScoreQuestion,
} from "./schema";
import {
  PROBABILITY_SUM_TOLERANCE,
  SCORE_WEIGHTED_MEAN_TOLERANCE,
} from "./schema";

export const InvariantViolation = {
  MissingAnswer: "missing_answer",
  ExtraAnswer: "extra_answer",
  TypeMismatch: "type_mismatch",
  ProbabilityOutOfRange: "probability_out_of_range",
  ProbabilitiesDoNotSum: "probabilities_do_not_sum",
  ChoiceNotInOptions: "choice_not_in_options",
  ProbabilityKeysMismatch: "probability_keys_mismatch",
  ArgmaxMismatch: "argmax_mismatch",
  LegendMismatch: "legend_mismatch",
  ScoreNotWeightedMean: "score_not_weighted_mean",
  ScoreOutOfRange: "score_out_of_range",
  ConfidenceOutOfRange: "confidence_out_of_range",
} as const;

export type InvariantViolation =
  (typeof InvariantViolation)[keyof typeof InvariantViolation];

function inUnitInterval(value: number): boolean {
  return Number.isFinite(value) && value >= 0 && value <= 1;
}

function sumsToOne(values: readonly number[]): boolean {
  const total = values.reduce((sum, value) => sum + value, 0);
  return Math.abs(total - 1) <= PROBABILITY_SUM_TOLERANCE;
}

function sameKeySet(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) {
    return false;
  }
  const set = new Set(a);
  return b.every((key) => set.has(key));
}

function argmaxKey(
  probabilities: Readonly<Record<string, number>>
): string | null {
  let best: string | null = null;
  let bestValue = Number.NEGATIVE_INFINITY;
  for (const [key, value] of Object.entries(probabilities)) {
    if (value > bestValue) {
      best = key;
      bestValue = value;
    }
  }
  return best;
}

function checkProbabilityValues(
  probabilities: Readonly<Record<string, number>>
): string[] {
  const values = Object.values(probabilities);
  const violations: string[] = [];
  if (!values.every(inUnitInterval)) {
    violations.push(InvariantViolation.ProbabilityOutOfRange);
  }
  if (!sumsToOne(values)) {
    violations.push(InvariantViolation.ProbabilitiesDoNotSum);
  }
  return violations;
}

function checkConfidence(confidence: number | undefined): string[] {
  return confidence !== undefined && !inUnitInterval(confidence)
    ? [InvariantViolation.ConfidenceOutOfRange]
    : [];
}

function checkChoice(
  question: ChoiceQuestion,
  answer: Extract<DecisionsAnswer, { type: "choice" }>
): string[] {
  const options = Object.keys(question.criteria);
  const violations: string[] = [];
  if (!options.includes(answer.choice)) {
    violations.push(InvariantViolation.ChoiceNotInOptions);
  }
  if (!sameKeySet(options, Object.keys(answer.probabilities))) {
    violations.push(InvariantViolation.ProbabilityKeysMismatch);
  }
  violations.push(...checkProbabilityValues(answer.probabilities));
  const top = argmaxKey(answer.probabilities);
  if (
    top !== null &&
    top !== answer.choice &&
    (answer.probabilities[top] ?? 0) -
      (answer.probabilities[answer.choice] ?? 0) >
      PROBABILITY_SUM_TOLERANCE
  ) {
    violations.push(InvariantViolation.ArgmaxMismatch);
  }
  violations.push(...checkConfidence(answer.confidence));
  return violations;
}

function checkScore(
  question: ScoreQuestion,
  answer: Extract<DecisionsAnswer, { type: "score" }>
): string[] {
  const levelKeys = question.criteria.map((_, index) => `${index}`);
  const violations: string[] = [];
  if (!sameKeySet(levelKeys, Object.keys(answer.legend))) {
    violations.push(InvariantViolation.LegendMismatch);
  }
  if (!sameKeySet(levelKeys, Object.keys(answer.probabilities))) {
    violations.push(InvariantViolation.ProbabilityKeysMismatch);
  }
  violations.push(...checkProbabilityValues(answer.probabilities));
  const maxIndex = Math.max(question.criteria.length - 1, 0);
  if (
    !Number.isFinite(answer.score) ||
    answer.score < 0 ||
    answer.score > maxIndex
  ) {
    violations.push(InvariantViolation.ScoreOutOfRange);
  }
  const weighted = levelKeys.reduce(
    (sum, key, index) => sum + index * (answer.probabilities[key] ?? 0),
    0
  );
  if (Math.abs(weighted - answer.score) > SCORE_WEIGHTED_MEAN_TOLERANCE) {
    violations.push(InvariantViolation.ScoreNotWeightedMean);
  }
  violations.push(...checkConfidence(answer.confidence));
  return violations;
}

export function checkAnswerInvariants(
  question: DecisionsQuestion,
  answer: DecisionsAnswer
): readonly string[] {
  if (question.type !== answer.type) {
    return [InvariantViolation.TypeMismatch];
  }
  switch (answer.type) {
    case "noul": {
      return inUnitInterval(answer.noul)
        ? []
        : [InvariantViolation.ProbabilityOutOfRange];
    }
    case "choice": {
      return question.type === "choice" ? checkChoice(question, answer) : [];
    }
    case "score": {
      return question.type === "score" ? checkScore(question, answer) : [];
    }
    default: {
      return answer satisfies never;
    }
  }
}

export function checkResponseInvariants(
  questions: Readonly<Record<string, DecisionsQuestion>>,
  response: DecisionsResponse
): Readonly<Record<string, readonly string[]>> {
  const result: Record<string, readonly string[]> = {};
  for (const [id, question] of Object.entries(questions)) {
    const answer = response.answers[id];
    result[id] =
      answer === undefined
        ? [InvariantViolation.MissingAnswer]
        : checkAnswerInvariants(question, answer);
  }
  for (const id of Object.keys(response.answers)) {
    if (!(id in questions)) {
      result[id] = [InvariantViolation.ExtraAnswer];
    }
  }
  return result;
}
