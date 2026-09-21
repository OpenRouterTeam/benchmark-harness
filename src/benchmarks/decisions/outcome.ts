import { Either } from "../../internal/either";
import type { ValueOf } from "../../internal/guards";
import { isRecord } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import { checkAnswerInvariants } from "./invariants";
import { NOUL_LABELS } from "./label-spaces";
import { argmaxLabel } from "./metrics";
import type {
  DecisionOutcome,
  DecisionsAnswer,
  DecisionsResponse,
  DecisionTaskSpec,
} from "./schema";
import { DecisionArm, DecisionOutcomeSchema } from "./schema";

const NOUL_TRUE_LABEL = NOUL_LABELS[0];

export const OutcomeViolation = {
  MissingAnswer: "missing_answer",
  UnparsableOutput: "unparsable_output",
  LabelNotInOptions: "label_not_in_options",
  MissingConfidence: "missing_confidence",
  ConfidenceBelowUniform: "confidence_below_uniform",
} as const;

export type OutcomeViolation = ValueOf<typeof OutcomeViolation>;

const SOFT_VIOLATIONS: ReadonlySet<string> = new Set([
  OutcomeViolation.MissingConfidence,
  OutcomeViolation.ConfidenceBelowUniform,
]);

export function isHardViolation(violation: string): boolean {
  return !SOFT_VIOLATIONS.has(violation);
}

export function answerDistribution(
  answer: DecisionsAnswer,
  labels: readonly string[]
): Readonly<Record<string, number>> {
  switch (answer.type) {
    case "noul": {
      return Object.fromEntries(
        labels.map((label) => [
          label,
          label === NOUL_TRUE_LABEL ? answer.noul : 1 - answer.noul,
        ])
      );
    }
    case "choice":
    case "score": {
      return Object.fromEntries(
        labels.map((label) => [label, answer.probabilities[label] ?? 0])
      );
    }
    default: {
      return answer satisfies never;
    }
  }
}

function answerConfidence(answer: DecisionsAnswer): number | null {
  switch (answer.type) {
    case "noul": {
      return null;
    }
    case "choice":
    case "score": {
      return answer.confidence ?? null;
    }
    default: {
      return answer satisfies never;
    }
  }
}

function answerScoreValue(answer: DecisionsAnswer): number | null {
  return answer.type === "score" ? answer.score : null;
}

export interface DecisionOutcomeInput {
  readonly spec: DecisionTaskSpec;
  readonly response: DecisionsResponse;
  readonly latencyMs: number;
}

export function decisionOutcomeFromResponse(
  input: DecisionOutcomeInput
): DecisionOutcome {
  const { spec, response } = input;
  const answer = response.answers[spec.questionId];
  const base = {
    arm: DecisionArm.Decision,
    model: response.model,
    provider: response.provider,
    latencyMs: input.latencyMs,
    cost: response.usage?.cost ?? null,
    inputTokens: response.usage?.input_tokens ?? null,
    outputTokens: response.usage?.output_tokens ?? null,
  } as const;
  if (answer === undefined) {
    return {
      ...base,
      distribution: {},
      argmax: null,
      confidence: null,
      scoreValue: null,
      violations: [OutcomeViolation.MissingAnswer],
      rawAnswer: null,
    };
  }
  const violations = checkAnswerInvariants(spec.question, answer);
  const distribution = answerDistribution(answer, spec.labels);
  return {
    ...base,
    distribution,
    argmax: argmaxLabel(spec.labels, distribution),
    confidence: answerConfidence(answer),
    scoreValue: answerScoreValue(answer),
    violations: [...violations],
    rawAnswer: answer,
  };
}

export interface LlmOutcomeInput {
  readonly spec: DecisionTaskSpec;
  readonly model: string;
  readonly completion: string;
  readonly latencyMs: number;
  readonly cost: number | null;
  readonly inputTokens: number | null;
  readonly outputTokens: number | null;
}

const ANSWER_KEY = "answer" as const;
const CONFIDENCE_KEY = "confidence" as const;

function lastJsonObject(text: string): Record<string, unknown> | undefined {
  const matches = text.match(/\{[^{}]*\}/gu);
  if (matches === null) {
    return undefined;
  }
  for (let i = matches.length - 1; i >= 0; i -= 1) {
    try {
      const parsed: unknown = JSON.parse(matches[i]!);
      if (isRecord(parsed)) {
        return parsed;
      }
    } catch {
      continue;
    }
  }
  return undefined;
}

function answerCandidate(raw: unknown): string | undefined {
  if (typeof raw === "string") {
    return raw.trim();
  }
  if (typeof raw === "number" || typeof raw === "boolean") {
    return `${raw}`;
  }
  return undefined;
}

function verbalizedConfidence(raw: unknown): number | null {
  return typeof raw === "number" && raw >= 0 && raw <= 1 ? raw : null;
}

export interface ExtractedLlmLabel {
  readonly label: string | null;
  readonly confidence: number | null;
  readonly violation: string | null;
}

export function extractLlmLabel(
  completion: string,
  labels: readonly string[]
): ExtractedLlmLabel {
  const json = lastJsonObject(completion);
  if (json === undefined) {
    return {
      label: null,
      confidence: null,
      violation: OutcomeViolation.UnparsableOutput,
    };
  }
  const candidate = answerCandidate(json[ANSWER_KEY]);
  const confidence = verbalizedConfidence(json[CONFIDENCE_KEY]);
  if (candidate === undefined) {
    return {
      label: null,
      confidence,
      violation: OutcomeViolation.UnparsableOutput,
    };
  }
  const exact = labels.find((label) => label === candidate);
  if (exact !== undefined) {
    return { label: exact, confidence, violation: null };
  }
  const folded = labels.find(
    (label) => label.toLowerCase() === candidate.toLowerCase()
  );
  if (folded !== undefined) {
    return { label: folded, confidence, violation: null };
  }
  return {
    label: null,
    confidence,
    violation: OutcomeViolation.LabelNotInOptions,
  };
}

export function verbalizedDistribution(
  labels: readonly string[],
  label: string,
  confidence: number
): Readonly<Record<string, number>> {
  const others = labels.length - 1;
  const rest = others === 0 ? 0 : (1 - confidence) / others;
  return Object.fromEntries(
    labels.map((entry) => [entry, entry === label ? confidence : rest])
  );
}

export function llmOutcomeFromCompletion(
  input: LlmOutcomeInput
): DecisionOutcome {
  const { spec } = input;
  const extracted = extractLlmLabel(input.completion, spec.labels);
  const confidence = extracted.confidence ?? 1;
  const distribution =
    extracted.label === null
      ? {}
      : verbalizedDistribution(spec.labels, extracted.label, confidence);
  const scoreValue =
    extracted.label !== null && spec.question.type === "score"
      ? Number(extracted.label)
      : null;
  const violations: string[] = [];
  if (extracted.violation !== null) {
    violations.push(extracted.violation);
  }
  if (extracted.label !== null && extracted.confidence === null) {
    violations.push(OutcomeViolation.MissingConfidence);
  }
  if (extracted.label !== null && confidence < 1 / spec.labels.length) {
    violations.push(OutcomeViolation.ConfidenceBelowUniform);
  }
  return {
    arm: DecisionArm.Llm,
    model: input.model,
    distribution,
    argmax: extracted.label,
    confidence: extracted.label === null ? null : extracted.confidence,
    scoreValue,
    latencyMs: input.latencyMs,
    cost: input.cost,
    inputTokens: input.inputTokens,
    outputTokens: input.outputTokens,
    violations,
    rawAnswer: input.completion,
  };
}

export function parseDecisionOutcome(value: unknown): DecisionOutcome | null {
  const parsed = parseSchema(DecisionOutcomeSchema, value);
  return Either.isLeft(parsed) ? null : parsed.right;
}
