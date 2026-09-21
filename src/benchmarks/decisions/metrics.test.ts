import { describe, expect, it } from "bun:test";

import {
  argmaxLabel,
  CONFIDENCE_THRESHOLD,
  expectedCalibrationError,
  expectedLevelIndex,
  mean,
  normalizeDistribution,
  observationMetrics,
  percentile,
  rankedProbabilityScore,
  selectiveMetrics,
} from "./metrics";

const LABELS = ["a", "b", "c"] as const;

describe("normalizeDistribution", () => {
  it("rescales positive mass to sum to one over the label set", () => {
    const result = normalizeDistribution(LABELS, { a: 2, b: 1, c: 1 });
    expect(result).toEqual({ a: 0.5, b: 0.25, c: 0.25 });
  });
  it("keeps zero mass as zero rather than inventing a uniform prior", () => {
    expect(normalizeDistribution(LABELS, {})).toEqual({ a: 0, b: 0, c: 0 });
  });
  it("clamps negative mass to zero and drops labels outside the set", () => {
    const result = normalizeDistribution(LABELS, { a: -1, b: 1, zzz: 5 });
    expect(result).toEqual({ a: 0, b: 1, c: 0 });
  });
});

describe("argmaxLabel", () => {
  it("returns the first label with the largest mass", () => {
    expect(argmaxLabel(LABELS, { a: 0.2, b: 0.5, c: 0.3 })).toBe("b");
  });
  it("breaks ties toward the earlier label", () => {
    expect(argmaxLabel(LABELS, { a: 0.5, b: 0.5, c: 0 })).toBe("a");
  });
  it("returns null for an empty label set", () => {
    expect(argmaxLabel([], {})).toBeNull();
  });
});

describe("observationMetrics", () => {
  it("scores a confident correct answer", () => {
    const metrics = observationMetrics({
      labels: LABELS,
      distribution: { a: 0.9, b: 0.1, c: 0 },
      gold: "a",
      goldKnowable: true,
      ordinal: false,
    });
    expect(metrics.correct).toBe(true);
    expect(metrics.argmax).toBe("a");
    expect(metrics.confidence).toBeCloseTo(0.9, 12);
    expect(metrics.pGold).toBeCloseTo(0.9, 12);
    expect(metrics.brier).toBeCloseTo(0.01 + 0.01, 12);
    expect(metrics.nll).toBeCloseTo(-Math.log(0.9), 12);
  });
  it("scores a confident wrong answer with a high Brier score", () => {
    const metrics = observationMetrics({
      labels: LABELS,
      distribution: { a: 0, b: 1, c: 0 },
      gold: "a",
      goldKnowable: true,
      ordinal: false,
    });
    expect(metrics.correct).toBe(false);
    expect(metrics.argmax).toBe("b");
    expect(metrics.brier).toBeCloseTo(2, 12);
    expect(metrics.pGold).toBe(0);
    expect(Number.isFinite(metrics.nll)).toBe(true);
  });
  it("treats zero total mass as no answer at all", () => {
    const metrics = observationMetrics({
      labels: LABELS,
      distribution: {},
      gold: "a",
      goldKnowable: true,
      ordinal: false,
    });
    expect(metrics.correct).toBe(false);
    expect(metrics.argmax).toBeNull();
    expect(metrics.confidence).toBe(0);
  });
  it("never marks an unknowable gold as correct", () => {
    const metrics = observationMetrics({
      labels: LABELS,
      distribution: { a: 1, b: 0, c: 0 },
      gold: "a",
      goldKnowable: false,
      ordinal: false,
    });
    expect(metrics.correct).toBe(false);
    expect(metrics.confidence).toBe(1);
  });
  it("reports ordinal metrics only for ordinal observations", () => {
    const ordinal = observationMetrics({
      labels: ["0", "1", "2"],
      distribution: { "0": 0, "1": 0.5, "2": 0.5 },
      gold: "2",
      goldKnowable: true,
      ordinal: true,
    });
    expect(ordinal.rps).not.toBeNull();
    expect(ordinal.absoluteLevelError).toBeCloseTo(0.5, 12);
    const nominal = observationMetrics({
      labels: LABELS,
      distribution: { a: 1, b: 0, c: 0 },
      gold: "a",
      goldKnowable: true,
      ordinal: false,
    });
    expect(nominal.rps).toBeNull();
    expect(nominal.absoluteLevelError).toBeNull();
  });
});

describe("expectedLevelIndex", () => {
  it("is the probability-weighted level index", () => {
    expect(
      expectedLevelIndex(["0", "1", "2"], { "0": 0.25, "1": 0.5, "2": 0.25 })
    ).toBeCloseTo(1, 12);
  });
});

describe("rankedProbabilityScore", () => {
  it("is zero for a point mass on the gold level", () => {
    expect(
      rankedProbabilityScore(["0", "1", "2"], { "0": 0, "1": 1, "2": 0 }, 1)
    ).toBe(0);
  });
  it("penalizes a point mass two levels away more than one level away", () => {
    const labels = ["0", "1", "2"];
    const near = rankedProbabilityScore(labels, { "0": 0, "1": 1, "2": 0 }, 0);
    const far = rankedProbabilityScore(labels, { "0": 0, "1": 0, "2": 1 }, 0);
    expect(far).toBeGreaterThan(near);
  });
});

describe("mean and percentile", () => {
  it("return null for an empty input", () => {
    expect(mean([])).toBeNull();
    expect(percentile([], 0.5)).toBeNull();
  });
  it("compute the arithmetic mean and nearest-rank percentiles", () => {
    expect(mean([1, 2, 3, 6])).toBe(3);
    expect(percentile([5, 1, 3], 0.5)).toBe(3);
    expect(percentile([5, 1, 3], 1)).toBe(5);
  });
});

describe("expectedCalibrationError", () => {
  it("is zero when confidence equals accuracy in every bin", () => {
    const confidences = [0.95, 0.95, 0.95, 0.95];
    const correct = [true, true, true, true];
    expect(expectedCalibrationError(confidences, correct)).toBeCloseTo(
      0.05,
      12
    );
  });
  it("is large when confident predictions are always wrong", () => {
    expect(expectedCalibrationError([0.99, 0.99], [false, false])).toBeCloseTo(
      0.99,
      12
    );
  });
  it("returns null with no observations", () => {
    expect(expectedCalibrationError([], [])).toBeNull();
  });
});

describe("selectiveMetrics", () => {
  it("returns null with no observations", () => {
    expect(selectiveMetrics([], [])).toBeNull();
  });
  it("measures coverage and accuracy above the confidence threshold", () => {
    const confidences = [0.95, 0.92, 0.6, 0.3];
    const correct = [true, false, true, false];
    const result = selectiveMetrics(confidences, correct);
    expect(result).not.toBeNull();
    expect(result?.coverageAtThreshold).toBeCloseTo(0.5, 12);
    expect(result?.accuracyAtThreshold).toBeCloseTo(0.5, 12);
    expect(result?.confidentErrorRate).toBeCloseTo(0.25, 12);
    expect(CONFIDENCE_THRESHOLD).toBe(0.9);
  });
  it("ranks by confidence for selective accuracy at fixed coverage", () => {
    const confidences = [0.9, 0.8, 0.7, 0.6];
    const correct = [true, true, false, false];
    const result = selectiveMetrics(confidences, correct);
    expect(result?.selectiveAccuracyAtCoverage["0.5"]).toBe(1);
    expect(result?.selectiveAccuracyAtCoverage["0.8"]).toBeCloseTo(2 / 3, 12);
    expect(result?.aurc).toBeGreaterThanOrEqual(0);
  });
});
