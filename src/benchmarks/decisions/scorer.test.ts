import { describe, expect, it } from "bun:test";

import { ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
import { InvariantViolation } from "./invariants";
import { OutcomeViolation } from "./outcome";
import {
  aggregateDecisionRecords,
  decisionRecords,
  decisionRunLevelScores,
} from "./run-level";
import type { DecisionOutcome, DecisionTaskSpec } from "./schema";
import {
  DECISION_OUTCOME_METADATA_KEY,
  DECISION_SPEC_METADATA_KEY,
  DecisionArm,
  DecisionTaskId,
  DecisionVariant,
} from "./schema";
import type { DecisionSampleRecord, DecisionScoreExplanation } from "./scorer";
import {
  decisionRecordFromMetadata,
  isDecisionCorrect,
  isWellFormed,
  observationFor,
  scoreDecisionRecord,
} from "./scorer";

const SPEC: DecisionTaskSpec = {
  task: DecisionTaskId.Xnli,
  variant: DecisionVariant.Base,
  language: "en",
  questionId: "relation",
  state: "s",
  stateText: "s",
  question: {
    type: "choice",
    instructions: "q",
    criteria: { entailment: "a", neutral: "b", contradiction: "c" },
  },
  labels: ["entailment", "neutral", "contradiction"],
  gold: "neutral",
  goldKnowable: true,
};

const SCORE_SPEC: DecisionTaskSpec = {
  ...SPEC,
  task: DecisionTaskId.Sst5,
  questionId: "sentiment",
  question: { type: "score", instructions: "q", criteria: ["a", "b", "c"] },
  labels: ["0", "1", "2"],
  gold: "2",
};

function outcome(
  distribution: Readonly<Record<string, number>>,
  overrides: Partial<DecisionOutcome> = {}
): DecisionOutcome {
  const [argmax] = Object.entries(distribution).sort(
    (a, b) => b[1] - a[1]
  )[0] ?? [null];
  return {
    arm: DecisionArm.Decision,
    model: "m",
    distribution,
    argmax,
    confidence: null,
    scoreValue: null,
    latencyMs: 100,
    cost: 0.001,
    inputTokens: 10,
    outputTokens: 0,
    violations: [],
    rawAnswer: null,
    ...overrides,
  };
}

function explanationOf(record: DecisionSampleRecord): DecisionScoreExplanation {
  const score = scoreDecisionRecord(record);
  const parsed: unknown = JSON.parse(score.explanation);
  return parsed as DecisionScoreExplanation;
}

describe("scoreDecisionRecord", () => {
  it("marks a confident correct argmax as correct and exposes the answer", () => {
    const record = {
      spec: SPEC,
      outcome: outcome({ entailment: 0.1, neutral: 0.8, contradiction: 0.1 }),
    };
    const score = scoreDecisionRecord(record);
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("neutral");
    const explanation = explanationOf(record);
    expect(explanation.correct).toBe(true);
    expect(explanation.confidence).toBeCloseTo(0.8, 12);
    expect(explanation.pGold).toBeCloseTo(0.8, 12);
    expect(explanation.rps).toBeNull();
    expect(explanation.violations).toEqual([]);
  });
  it("marks a wrong argmax incorrect", () => {
    const record = {
      spec: SPEC,
      outcome: outcome({ entailment: 0.7, neutral: 0.2, contradiction: 0.1 }),
    };
    expect(scoreDecisionRecord(record).value).toBe(ScoreValue.Incorrect);
    expect(scoreDecisionRecord(record).answer).toBe("entailment");
  });
  it("marks a correct argmax incorrect when a hard invariant violation is present", () => {
    const record = {
      spec: SPEC,
      outcome: outcome(
        { entailment: 0.1, neutral: 0.8, contradiction: 0.1 },
        { violations: [InvariantViolation.ProbabilitiesDoNotSum] }
      ),
    };
    expect(isWellFormed(record)).toBe(false);
    expect(scoreDecisionRecord(record).value).toBe(ScoreValue.Incorrect);
  });
  it("keeps a correct argmax correct under a soft violation", () => {
    const record = {
      spec: SPEC,
      outcome: outcome(
        { entailment: 0, neutral: 1, contradiction: 0 },
        {
          arm: DecisionArm.Llm,
          violations: [OutcomeViolation.MissingConfidence],
        }
      ),
    };
    expect(isWellFormed(record)).toBe(true);
    expect(scoreDecisionRecord(record).value).toBe(ScoreValue.Correct);
  });
  it("credits evidence-removed items only for low confidence", () => {
    const spec = { ...SPEC, goldKnowable: false };
    const hedged = {
      spec,
      outcome: outcome({
        entailment: 0.34,
        neutral: 0.33,
        contradiction: 0.33,
      }),
    };
    const confident = {
      spec,
      outcome: outcome({
        entailment: 0.02,
        neutral: 0.96,
        contradiction: 0.02,
      }),
    };
    expect(scoreDecisionRecord(hedged).value).toBe(ScoreValue.Correct);
    expect(scoreDecisionRecord(confident).value).toBe(ScoreValue.Incorrect);
    expect(isDecisionCorrect(confident, observationFor(confident))).toBe(false);
  });
  it("reports ordinal metrics for score questions", () => {
    const record = {
      spec: SCORE_SPEC,
      outcome: outcome({ "0": 0, "1": 0.5, "2": 0.5 }),
    };
    const explanation = explanationOf(record);
    expect(explanation.rps).not.toBeNull();
    expect(explanation.absoluteLevelError).toBeCloseTo(0.5, 12);
  });
  it("omits ordinal metrics for shuffled score rubrics", () => {
    const record = {
      spec: { ...SCORE_SPEC, variant: DecisionVariant.Shuffled },
      outcome: outcome({ "0": 0, "1": 0.5, "2": 0.5 }),
    };
    const explanation = explanationOf(record);
    expect(explanation.rps).toBeNull();
    expect(explanation.absoluteLevelError).toBeNull();
    const reversed = explanationOf({
      ...record,
      spec: { ...SCORE_SPEC, variant: DecisionVariant.Reversed },
    });
    expect(reversed.absoluteLevelError).toBeCloseTo(0.5, 12);
  });
});

describe("decisionRecordFromMetadata", () => {
  it("returns null without metadata, a spec, or an outcome", () => {
    expect(decisionRecordFromMetadata(undefined)).toBeNull();
    expect(
      decisionRecordFromMetadata({ [DECISION_SPEC_METADATA_KEY]: SPEC })
    ).toBeNull();
    expect(
      decisionRecordFromMetadata({
        [DECISION_OUTCOME_METADATA_KEY]: outcome({ neutral: 1 }),
      })
    ).toBeNull();
  });
  it("parses a complete record", () => {
    const record = decisionRecordFromMetadata({
      [DECISION_SPEC_METADATA_KEY]: SPEC,
      [DECISION_OUTCOME_METADATA_KEY]: outcome({ neutral: 1 }),
    });
    expect(record?.spec.gold).toBe("neutral");
    expect(record?.outcome.argmax).toBe("neutral");
  });
});

function runResultWith(records: readonly DecisionSampleRecord[]): RunResult {
  return {
    metrics: {
      accuracy: 0,
      totalQuestions: records.length,
      correctAnswers: 0,
      skippedQuestions: 0,
    },
    usage: {
      inputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      reasoningTokens: 0,
      totalCost: 0,
      generationTimeMs: 0,
    },
    sampleScores: records.map((record, index) => ({
      sampleId: `s${index}`,
      epoch: 0,
      score: scoreDecisionRecord(record),
      metadata: {
        [DECISION_SPEC_METADATA_KEY]: record.spec,
        [DECISION_OUTCOME_METADATA_KEY]: record.outcome,
      },
    })),
  };
}

describe("aggregateDecisionRecords", () => {
  const records: DecisionSampleRecord[] = [
    {
      spec: SPEC,
      outcome: outcome({ entailment: 0.05, neutral: 0.9, contradiction: 0.05 }),
    },
    {
      spec: SPEC,
      outcome: outcome(
        { entailment: 0.95, neutral: 0.025, contradiction: 0.025 },
        {
          latencyMs: 300,
        }
      ),
    },
    {
      spec: SPEC,
      outcome: outcome(
        { entailment: 0.2, neutral: 0.6, contradiction: 0.2 },
        {
          cost: null,
          violations: [OutcomeViolation.MissingConfidence],
          arm: DecisionArm.Llm,
        }
      ),
    },
    {
      spec: { ...SPEC, goldKnowable: false },
      outcome: outcome({
        entailment: 0.02,
        neutral: 0.96,
        contradiction: 0.02,
      }),
    },
  ];
  it("computes accuracy, calibration, selective, latency, usage, and violation metrics", () => {
    const { metrics } = aggregateDecisionRecords("all", records);
    expect(metrics.samples?.value).toBe(4);
    expect(metrics.scored_samples?.value).toBe(3);
    expect(metrics.accuracy?.value).toBeCloseTo(2 / 3, 12);
    expect(metrics.mean_confidence?.value).toBeCloseTo(
      (0.9 + 0.95 + 0.6) / 3,
      12
    );
    expect(metrics.confidence_bias?.value).toBeCloseTo(
      (0.9 + 0.95 + 0.6) / 3 - 2 / 3,
      12
    );
    expect(metrics.coverage_at_threshold?.value).toBeCloseTo(2 / 3, 12);
    expect(metrics.accuracy_at_threshold?.value).toBeCloseTo(0.5, 12);
    expect(metrics.confident_error_rate?.value).toBeCloseTo(1 / 3, 12);
    expect(metrics.evidence_removed_mean_confidence?.value).toBeCloseTo(
      0.96,
      12
    );
    expect(metrics.evidence_removed_confident_rate?.value).toBe(1);
    expect(metrics.latency_ms_p50?.value).toBe(100);
    expect(metrics.latency_ms_p95?.value).toBe(300);
    expect(metrics.total_cost?.value).toBeCloseTo(0.003, 12);
    expect(metrics.cost_known_samples?.value).toBe(3);
    expect(metrics.violation_rate?.value).toBeCloseTo(0.25, 12);
    expect(metrics.hard_violation_rate?.value).toBe(0);
    expect(metrics.ranked_probability_score).toBeUndefined();
    expect(metrics.score_mae).toBeUndefined();
  });
  it("omits metrics that cannot be computed from an empty group", () => {
    const { metrics } = aggregateDecisionRecords("empty", []);
    expect(metrics.samples?.value).toBe(0);
    expect(metrics.accuracy).toBeUndefined();
    expect(metrics.total_cost).toBeUndefined();
    expect(metrics.violation_rate).toBeUndefined();
  });
  it("reports ordinal metrics for score groups", () => {
    const { metrics } = aggregateDecisionRecords("score", [
      { spec: SCORE_SPEC, outcome: outcome({ "0": 0, "1": 0.5, "2": 0.5 }) },
    ]);
    expect(metrics.score_mae?.value).toBeCloseTo(0.5, 12);
    expect(metrics.ranked_probability_score?.value).toBeGreaterThan(0);
  });
});

describe("decisionRunLevelScores", () => {
  it("emits an overall group plus one group per task, language, variant, and arm", () => {
    const records: DecisionSampleRecord[] = [
      { spec: SPEC, outcome: outcome({ neutral: 1 }) },
      {
        spec: SPEC,
        outcome: outcome({ neutral: 1 }, { arm: DecisionArm.Llm }),
      },
      {
        spec: { ...SPEC, variant: DecisionVariant.Shuffled },
        outcome: outcome({ neutral: 1 }),
      },
    ];
    const result = runResultWith(records);
    expect(decisionRecords(result.sampleScores)).toHaveLength(3);
    const scores = decisionRunLevelScores(result);
    expect(scores.map((score) => score.name)).toEqual([
      "decisions",
      "decisions_xnli_en_base_decision",
      "decisions_xnli_en_base_llm",
      "decisions_xnli_en_shuffled_decision",
    ]);
    expect(scores[0]?.metrics.samples?.value).toBe(3);
    expect(scores[1]?.metrics.samples?.value).toBe(1);
  });
  it("ignores sample scores without decision metadata", () => {
    const result = runResultWith([]);
    const withForeign: RunResult = {
      ...result,
      sampleScores: [
        {
          sampleId: "x",
          epoch: 0,
          score: { value: ScoreValue.Incorrect, answer: null, explanation: "" },
        },
      ],
    };
    expect(decisionRecords(withForeign.sampleScores)).toEqual([]);
    expect(decisionRunLevelScores(withForeign)).toHaveLength(1);
  });
});
