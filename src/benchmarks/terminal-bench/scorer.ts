import { succeed } from "effect/Effect";

import type { Score, Target, TaskState } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { readTerminalBenchMeta } from "./dataset";

function readReward(state: TaskState): {
  reward: number;
  testOutput?: string;
} {
  const meta = readTerminalBenchMeta(state.sample.metadata);
  if (meta === undefined) {
    return { reward: 0 };
  }
  return {
    reward: meta.reward ?? 0,
    ...(meta.testOutput !== undefined && { testOutput: meta.testOutput }),
  };
}

export const terminalBenchScorer: ScorerService = (
  state: TaskState,
  target: Target
) => {
  const { reward, testOutput } = readReward(state);
  const score: Score = {
    value: reward >= 1 ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: target.text,
    explanation: testOutput ?? "",
  };
  return succeed(score);
};
