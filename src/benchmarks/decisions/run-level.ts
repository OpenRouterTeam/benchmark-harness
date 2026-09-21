import type { SampleScore } from "../../harness/metric";
import type { RunResult } from "../../harness/run";
import { definedValues } from "../../internal/guards";
import {
  CONFIDENCE_THRESHOLD,
  expectedCalibrationError,
  mean,
  percentile,
  selectiveMetrics,
} from "./metrics";
import { DECISIONS_BENCHMARK_ID } from "./schema";
import type { DecisionSampleRecord } from "./scorer";
import {
  decisionRecordFromMetadata,
  isDecisionCorrect,
  isWellFormed,
  observationFor,
} from "./scorer";

export type RunLevelScore = {
  readonly name: string;
  readonly metrics: Readonly<Record<string, { readonly value: number }>>;
};

function groupKey(record: DecisionSampleRecord): string {
  return `${DECISIONS_BENCHMARK_ID}_${record.spec.task}_${record.spec.language}_${record.spec.variant}_${record.outcome.arm}`;
}

function toMetrics(
  values: Readonly<Record<string, number | null | undefined>>
): Readonly<Record<string, { readonly value: number }>> {
  return Object.fromEntries(
    Object.entries(definedValues(values))
      .filter((entry): entry is [string, number] => entry[1] !== null)
      .map(([name, value]) => [name, { value }])
  );
}

export function aggregateDecisionRecords(
  name: string,
  records: readonly DecisionSampleRecord[]
): RunLevelScore {
  const knowable = records.filter((record) => record.spec.goldKnowable);
  const observations = knowable.map((record) => observationFor(record));
  const confidences = observations.map((observation) => observation.confidence);
  const correct = observations.map((observation, index) =>
    isDecisionCorrect(knowable[index]!, observation)
  );
  const accuracy = mean(correct.map((value) => (value ? 1 : 0)));
  const meanConfidence = mean(confidences);
  const selective = selectiveMetrics(confidences, correct);
  const unknowable = records.filter((record) => !record.spec.goldKnowable);
  const unknowableConfidence = unknowable.map(
    (record) => observationFor(record).confidence
  );
  const latencies = records.map((record) => record.outcome.latencyMs);
  const violationCount = records.filter(
    (record) => record.outcome.violations.length > 0
  ).length;
  const hardViolationCount = records.filter(
    (record) => !isWellFormed(record)
  ).length;
  const knownCosts = records
    .map((record) => record.outcome.cost)
    .filter((value): value is number => value !== null);
  let totalCost = 0;
  for (const cost of knownCosts) {
    totalCost += cost;
  }
  const rps = observations
    .map((observation) => observation.rps)
    .filter((value): value is number => value !== null);
  const levelErrors = observations
    .map((observation) => observation.absoluteLevelError)
    .filter((value): value is number => value !== null);
  return {
    name,
    metrics: toMetrics({
      samples: records.length,
      scored_samples: knowable.length,
      accuracy,
      nll: mean(observations.map((observation) => observation.nll)),
      brier: mean(observations.map((observation) => observation.brier)),
      ece: expectedCalibrationError(confidences, correct),
      mean_confidence: meanConfidence,
      confidence_bias:
        accuracy === null || meanConfidence === null
          ? null
          : meanConfidence - accuracy,
      coverage_at_threshold: selective?.coverageAtThreshold,
      accuracy_at_threshold: selective?.accuracyAtThreshold,
      confident_error_rate: selective?.confidentErrorRate,
      selective_accuracy_at_50: selective?.selectiveAccuracyAtCoverage["0.5"],
      selective_accuracy_at_80: selective?.selectiveAccuracyAtCoverage["0.8"],
      aurc: selective?.aurc,
      confidence_threshold: CONFIDENCE_THRESHOLD,
      ranked_probability_score: rps.length > 0 ? mean(rps) : null,
      score_mae: levelErrors.length > 0 ? mean(levelErrors) : null,
      evidence_removed_mean_confidence:
        unknowableConfidence.length > 0 ? mean(unknowableConfidence) : null,
      evidence_removed_confident_rate:
        unknowableConfidence.length > 0
          ? mean(
              unknowableConfidence.map((confidence) =>
                confidence >= CONFIDENCE_THRESHOLD ? 1 : 0
              )
            )
          : null,
      latency_ms_p50: percentile(latencies, 0.5),
      latency_ms_p95: percentile(latencies, 0.95),
      mean_input_tokens: mean(
        records
          .map((record) => record.outcome.inputTokens)
          .filter((value): value is number => value !== null)
      ),
      mean_output_tokens: mean(
        records
          .map((record) => record.outcome.outputTokens)
          .filter((value): value is number => value !== null)
      ),
      total_cost: knownCosts.length > 0 ? totalCost : null,
      cost_known_samples: knownCosts.length,
      violation_rate:
        records.length > 0 ? violationCount / records.length : null,
      hard_violation_rate:
        records.length > 0 ? hardViolationCount / records.length : null,
    }),
  };
}

export function decisionRecords(
  sampleScores: readonly SampleScore[]
): readonly DecisionSampleRecord[] {
  return sampleScores.flatMap((sampleScore) => {
    const record = decisionRecordFromMetadata(sampleScore.metadata);
    return record === null ? [] : [record];
  });
}

export function decisionRunLevelScores(
  result: RunResult
): readonly RunLevelScore[] {
  const records = decisionRecords(result.sampleScores);
  const groups = new Map<string, DecisionSampleRecord[]>();
  for (const record of records) {
    const key = groupKey(record);
    const current = groups.get(key);
    if (current === undefined) {
      groups.set(key, [record]);
    } else {
      current.push(record);
    }
  }
  return [
    aggregateDecisionRecords(DECISIONS_BENCHMARK_ID, records),
    ...[...groups.entries()].map(([name, group]) =>
      aggregateDecisionRecords(name, group)
    ),
  ];
}
