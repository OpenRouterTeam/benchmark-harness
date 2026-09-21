import { succeed } from "effect/Effect";

import type { Score, TaskState } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import type { ObservationMetrics } from "./metrics";
import { CONFIDENCE_THRESHOLD, observationMetrics } from "./metrics";
import { isHardViolation, parseDecisionOutcome } from "./outcome";
import type { DecisionOutcome, DecisionTaskSpec } from "./schema";
import {
  DECISION_OUTCOME_METADATA_KEY,
  DECISION_SPEC_METADATA_KEY,
  DecisionTaskSpecSchema,
} from "./schema";

export interface DecisionSampleRecord {
  readonly spec: DecisionTaskSpec;
  readonly outcome: DecisionOutcome;
}

export function decisionRecordFromMetadata(
  metadata: Readonly<Record<string, unknown>> | undefined
): DecisionSampleRecord | null {
  if (metadata === undefined) {
    return null;
  }
  const spec = parseSchema(
    DecisionTaskSpecSchema,
    metadata[DECISION_SPEC_METADATA_KEY]
  );
  const outcome = parseDecisionOutcome(metadata[DECISION_OUTCOME_METADATA_KEY]);
  if (Either.isLeft(spec) || outcome === null) {
    return null;
  }
  return { spec: spec.right, outcome };
}

export function observationFor(
  record: DecisionSampleRecord
): ObservationMetrics {
  return observationMetrics({
    labels: record.spec.labels,
    distribution: record.outcome.distribution,
    gold: record.spec.gold,
    goldKnowable: record.spec.goldKnowable,
    ordinal: record.spec.question.type === "score",
  });
}

export function isWellFormed(record: DecisionSampleRecord): boolean {
  return !record.outcome.violations.some(isHardViolation);
}

export function isDecisionCorrect(
  record: DecisionSampleRecord,
  metrics: ObservationMetrics
): boolean {
  const wellFormed = isWellFormed(record);
  return record.spec.goldKnowable
    ? metrics.correct && wellFormed
    : metrics.confidence < CONFIDENCE_THRESHOLD && wellFormed;
}

export interface DecisionScoreExplanation {
  readonly correct: boolean;
  readonly goldKnowable: boolean;
  readonly gold: string;
  readonly argmax: string | null;
  readonly confidence: number;
  readonly pGold: number;
  readonly brier: number;
  readonly nll: number;
  readonly rps: number | null;
  readonly absoluteLevelError: number | null;
  readonly violations: readonly string[];
  readonly latencyMs: number;
}

export function scoreDecisionRecord(record: DecisionSampleRecord): Score {
  const metrics = observationFor(record);
  const correct = isDecisionCorrect(record, metrics);
  const explanation: DecisionScoreExplanation = {
    correct,
    goldKnowable: record.spec.goldKnowable,
    gold: record.spec.gold,
    argmax: metrics.argmax,
    confidence: metrics.confidence,
    pGold: metrics.pGold,
    brier: metrics.brier,
    nll: metrics.nll,
    rps: metrics.rps,
    absoluteLevelError: metrics.absoluteLevelError,
    violations: record.outcome.violations,
    latencyMs: record.outcome.latencyMs,
  };
  return {
    value: correct ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: metrics.argmax,
    explanation: JSON.stringify(explanation),
  };
}

export const decisionScorer: ScorerService = (state: TaskState) => {
  const record = decisionRecordFromMetadata(state.sample.metadata);
  if (record === null) {
    return succeed({
      value: ScoreValue.Incorrect,
      answer: null,
      explanation: "No decision outcome recorded for this sample.",
    });
  }
  return succeed(scoreDecisionRecord(record));
};
