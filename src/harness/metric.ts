import type { ChatMessage, ResponseItem, Score, ScoreValue } from "./core";
import { ScoreValue as SV, scoreToNumber } from "./core";

export interface SampleScore {
  readonly sampleId: string;
  readonly epoch: number;
  readonly score: Score;
  readonly messages?: readonly ChatMessage[];
  readonly responseItems?: readonly ResponseItem[];
  readonly requestBody?: Readonly<Record<string, unknown>>;
  readonly generationIds?: readonly string[];
  readonly metadata?: Readonly<Record<string, unknown>>;
  readonly input?: string;
  readonly target?: string;
}

export function meanEpochReducer(scores: readonly Score[]): number {
  if (scores.length === 0) {
    return 0;
  }
  const sum = scores.reduce((acc, s) => acc + scoreToNumber(s.value), 0);
  return sum / scores.length;
}

export interface AggregateMetrics {
  readonly accuracy: number;
  readonly totalQuestions: number;
  readonly correctAnswers: number;
  readonly skippedQuestions: number;
}

export function aggregateScores(
  sampleScores: readonly SampleScore[]
): AggregateMetrics {
  const bySample = new Map<string, Score[]>();
  for (const { sampleId, score } of sampleScores) {
    const existing = bySample.get(sampleId);
    if (existing) {
      existing.push(score);
    } else {
      bySample.set(sampleId, [score]);
    }
  }
  const perSampleValues: number[] = [];
  let skippedQuestions = 0;
  for (const scores of bySample.values()) {
    const evaluated = scores.filter((s) => s.value !== SV.Skipped);
    if (evaluated.length === 0) {
      skippedQuestions++;
    } else {
      perSampleValues.push(meanEpochReducer(evaluated));
    }
  }
  const totalQuestions = perSampleValues.length;
  if (totalQuestions === 0) {
    return {
      accuracy: 0,
      totalQuestions: 0,
      correctAnswers: 0,
      skippedQuestions,
    };
  }
  const accuracy =
    perSampleValues.reduce((acc, v) => acc + v, 0) / perSampleValues.length;
  const correctAnswers = perSampleValues.filter((value) => value >= 0.5).length;
  return {
    accuracy,
    totalQuestions,
    correctAnswers,
    skippedQuestions,
  };
}

export function countByValue(
  sampleScores: readonly SampleScore[]
): Record<ScoreValue, number> {
  const counts: Record<ScoreValue, number> = {
    [SV.Correct]: 0,
    [SV.Incorrect]: 0,
    [SV.Skipped]: 0,
  };
  for (const { score } of sampleScores) {
    counts[score.value]++;
  }
  return counts;
}
