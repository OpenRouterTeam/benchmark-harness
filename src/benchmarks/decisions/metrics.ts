export const PROBABILITY_EPSILON = 1e-12;

export const ECE_BIN_COUNT = 15;

export const CONFIDENCE_THRESHOLD = 0.9;

export const SELECTIVE_COVERAGES = [0.5, 0.8] as const;

export interface ProbabilisticObservation {
  readonly labels: readonly string[];
  readonly distribution: Readonly<Record<string, number>>;
  readonly gold: string;
  readonly goldKnowable: boolean;
  readonly ordinal: boolean;
}

export interface ObservationMetrics {
  readonly correct: boolean;
  readonly argmax: string | null;
  readonly confidence: number;
  readonly pGold: number;
  readonly brier: number;
  readonly nll: number;
  readonly rps: number | null;
  readonly absoluteLevelError: number | null;
}

export function normalizeDistribution(
  labels: readonly string[],
  distribution: Readonly<Record<string, number>>
): Record<string, number> {
  const raw = labels.map((label) => Math.max(distribution[label] ?? 0, 0));
  let total = 0;
  for (const value of raw) {
    total += value;
  }
  if (total <= 0) {
    return Object.fromEntries(labels.map((label) => [label, 0]));
  }
  return Object.fromEntries(
    labels.map((label, index) => [label, raw[index]! / total])
  );
}

export function argmaxLabel(
  labels: readonly string[],
  distribution: Readonly<Record<string, number>>
): string | null {
  let best: string | null = null;
  let bestValue = Number.NEGATIVE_INFINITY;
  for (const label of labels) {
    const value = distribution[label];
    if (value !== undefined && value > bestValue) {
      best = label;
      bestValue = value;
    }
  }
  return best;
}

export function observationMetrics(
  observation: ProbabilisticObservation
): ObservationMetrics {
  const { labels, gold } = observation;
  const distribution = normalizeDistribution(labels, observation.distribution);
  const argmax = argmaxLabel(labels, distribution);
  const pGold = distribution[gold] ?? 0;
  const anyMass = argmax !== null && (distribution[argmax] ?? 0) > 0;
  const confidence = argmax === null ? 0 : (distribution[argmax] ?? 0);
  let brier = 0;
  for (const label of labels) {
    const target = label === gold ? 1 : 0;
    brier += ((distribution[label] ?? 0) - target) ** 2;
  }
  const goldIndex = labels.indexOf(gold);
  const ordered = observation.ordinal && goldIndex !== -1;
  return {
    correct: observation.goldKnowable && anyMass && argmax === gold,
    argmax: anyMass ? argmax : null,
    confidence,
    pGold,
    brier,
    nll: -Math.log(Math.max(pGold, PROBABILITY_EPSILON)),
    rps: ordered
      ? rankedProbabilityScore(labels, distribution, goldIndex)
      : null,
    absoluteLevelError: ordered
      ? Math.abs(expectedLevelIndex(labels, distribution) - goldIndex)
      : null,
  };
}

export function expectedLevelIndex(
  labels: readonly string[],
  distribution: Readonly<Record<string, number>>
): number {
  let sum = 0;
  for (const [index, label] of labels.entries()) {
    sum += index * (distribution[label] ?? 0);
  }
  return sum;
}

export function rankedProbabilityScore(
  labels: readonly string[],
  distribution: Readonly<Record<string, number>>,
  goldIndex: number
): number {
  let cumulativeForecast = 0;
  let cumulativeTarget = 0;
  let total = 0;
  for (const [index, label] of labels.entries()) {
    cumulativeForecast += distribution[label] ?? 0;
    cumulativeTarget += index === goldIndex ? 1 : 0;
    total += (cumulativeForecast - cumulativeTarget) ** 2;
  }
  return total / Math.max(labels.length - 1, 1);
}

export function mean(values: readonly number[]): number | null {
  if (values.length === 0) {
    return null;
  }
  let sum = 0;
  for (const value of values) {
    sum += value;
  }
  return sum / values.length;
}

export function percentile(
  values: readonly number[],
  p: number
): number | null {
  if (values.length === 0) {
    return null;
  }
  const sorted = [...values].sort((a, b) => a - b);
  const position = Math.min(
    sorted.length - 1,
    Math.max(0, Math.ceil(p * sorted.length) - 1)
  );
  return sorted[position]!;
}

export function expectedCalibrationError(
  confidences: readonly number[],
  correct: readonly boolean[],
  bins: number = ECE_BIN_COUNT
): number | null {
  if (confidences.length === 0 || confidences.length !== correct.length) {
    return null;
  }
  const counts = new Array<number>(bins).fill(0);
  const confidenceSums = new Array<number>(bins).fill(0);
  const correctSums = new Array<number>(bins).fill(0);
  for (const [index, confidence] of confidences.entries()) {
    const bin = Math.min(bins - 1, Math.max(0, Math.floor(confidence * bins)));
    counts[bin]! += 1;
    confidenceSums[bin]! += confidence;
    correctSums[bin]! += correct[index] ? 1 : 0;
  }
  let ece = 0;
  for (let bin = 0; bin < bins; bin += 1) {
    const count = counts[bin]!;
    if (count === 0) {
      continue;
    }
    ece +=
      (count / confidences.length) *
      Math.abs(confidenceSums[bin]! / count - correctSums[bin]! / count);
  }
  return ece;
}

export interface SelectiveMetrics {
  readonly coverageAtThreshold: number;
  readonly accuracyAtThreshold: number | null;
  readonly confidentErrorRate: number;
  readonly selectiveAccuracyAtCoverage: Readonly<Record<string, number | null>>;
  readonly aurc: number | null;
}

export function selectiveMetrics(
  confidences: readonly number[],
  correct: readonly boolean[],
  threshold: number = CONFIDENCE_THRESHOLD
): SelectiveMetrics | null {
  const n = confidences.length;
  if (n === 0 || n !== correct.length) {
    return null;
  }
  const accepted = confidences
    .map((confidence, index) => ({ confidence, correct: correct[index]! }))
    .filter((entry) => entry.confidence >= threshold);
  const acceptedCorrect = accepted.filter((entry) => entry.correct).length;
  const ordered = confidences
    .map((confidence, index) => ({ confidence, correct: correct[index]! }))
    .sort((a, b) => b.confidence - a.confidence);
  let running = 0;
  let riskSum = 0;
  const selectiveAccuracy: Record<string, number | null> = {};
  for (const [index, entry] of ordered.entries()) {
    running += entry.correct ? 1 : 0;
    riskSum += 1 - running / (index + 1);
  }
  for (const coverage of SELECTIVE_COVERAGES) {
    const count = Math.max(1, Math.round(coverage * n));
    const subset = ordered.slice(0, count);
    selectiveAccuracy[`${coverage}`] =
      subset.filter((entry) => entry.correct).length / subset.length;
  }
  return {
    coverageAtThreshold: accepted.length / n,
    accuracyAtThreshold:
      accepted.length === 0 ? null : acceptedCorrect / accepted.length,
    confidentErrorRate: (accepted.length - acceptedCorrect) / n,
    selectiveAccuracyAtCoverage: selectiveAccuracy,
    aurc: riskSum / n,
  };
}
