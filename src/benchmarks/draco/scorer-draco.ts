import type { Effect } from "effect/Effect";
import { succeed } from "effect/Effect";

import type { Score, Target, TaskState } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import { Either } from "../../internal/either";
import { definedValues, isRecord } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { DracoPanelConfig, TaskScore } from "./schemas";
import { CriterionSchema, JudgeRunSchema } from "./schemas";
import { aggregateTaskScores } from "./scorer";

export function dracoScorer(
  config: DracoPanelConfig
): (state: TaskState, target: Target) => Effect<Score, never> {
  return (state, _target) => {
    const sample = state.sample;
    const criteria = readMetadataArray(
      sample.metadata?.["criteria"],
      CriterionSchema.array()
    );
    const verdicts = readMetadataArray(
      sample.metadata?.["verdicts"],
      JudgeRunSchema.array()
    );
    const generationCost = readGenerationCost(sample.metadata?.["generation"]);
    const taskScore = aggregateTaskScores(
      {
        id: sample.id,
        problem: sample.input,
        domain: String(sample.metadata?.["domain"] ?? "Unknown"),
        criteria,
        rawAnswer: {},
      },
      {
        taskId: sample.id,
        experimentName: config.name,
        judgeModel: config.judgeModel,
        targetRuns: config.judgeRuns,
        runs: verdicts,
      },
      config.name
    );
    const totalCost = (generationCost ?? 0) + taskScore.judgingCost;
    const fullScore: TaskScore = {
      ...taskScore,
      ...definedValues({
        generationCost: generationCost !== null ? generationCost : undefined,
      }),
      totalCost,
    };
    return succeed(
      definedValues({
        value:
          fullScore.meanNormalized >= 50
            ? ScoreValue.Correct
            : ScoreValue.Incorrect,
        answer: state.output?.completion ?? null,
        explanation: JSON.stringify({
          normalized: fullScore.meanNormalized,
          stdNormalized: fullScore.stdNormalized,
          passRate: fullScore.meanPassRate,
          judgeRunsCompleted: fullScore.judgeRunsCompleted,
          judgeRunsFailed: fullScore.judgeRunsFailed,
          runScores: fullScore.runScores,
          generationCost: fullScore.generationCost,
          judgingCost: fullScore.judgingCost,
          totalCost: fullScore.totalCost,
        }),
        trajectory:
          verdicts.length > 0
            ? ({ kind: "judge_runs", runs: verdicts } as const)
            : undefined,
      })
    );
  };
}

function readMetadataArray<T>(raw: unknown, schema: z.ZodType<T[]>): T[] {
  if (raw === undefined) {
    return [];
  }
  const parsed = parseSchema(schema, raw);
  if (Either.isLeft(parsed)) {
    return [];
  }
  return parsed.right;
}

function readGenerationCost(raw: unknown): number | null {
  if (!isRecord(raw)) {
    return null;
  }
  const cost = raw["cost"];
  return typeof cost === "number" ? cost : null;
}
