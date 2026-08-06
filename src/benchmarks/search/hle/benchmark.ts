import type { Effect } from "effect/Effect";

import type { Score, Target, TaskState } from "../../../harness/core";
import type { RunResult } from "../../../harness/run";
import type { SolverService } from "../../../harness/solver";
import type { ResponsesService } from "../../../providers/responses-client";
import { HLE_META } from "../../benchmark-meta";
import type { Benchmark } from "../../types";
import { makeSearchBenchmarkLayer } from "../core/benchmark";
import { DEEP_RESEARCH_INSTRUCTIONS } from "../core/prompts";
import type { SearchSolverOptions } from "../core/solver";
import {
  answerEquivalenceRunLevelScores,
  answerEquivalenceScorer,
  makeAnswerEquivalenceSolver,
} from "../grading/answer-equivalence-benchmark";
import { makeHleDatasetLayer } from "./dataset";

export const HLE_BENCHMARK_ID = HLE_META.id;

const HLE_TEMPERATURE = 0;

export function makeHleSolver(
  responses: ResponsesService,
  options: SearchSolverOptions
): SolverService {
  return makeAnswerEquivalenceSolver(responses, options);
}

export function hleScorer(
  state: TaskState,
  target: Target
): Effect<Score, never> {
  return answerEquivalenceScorer(state, target);
}

export const HLE_BENCHMARK: Benchmark = {
  id: HLE_BENCHMARK_ID,
  makeDatasetLayer: makeHleDatasetLayer,
  temperature: HLE_TEMPERATURE,
  defaultEpochs: HLE_META.defaultEpochs,
  makeLayer: (input) =>
    makeSearchBenchmarkLayer(input, {
      benchmarkId: HLE_BENCHMARK_ID,
      instructions: DEEP_RESEARCH_INSTRUCTIONS,
      temperature: HLE_TEMPERATURE,
      makeDatasetLayer: makeHleDatasetLayer,
      makeSolver: makeHleSolver,
      scorer: hleScorer,
    }),
  runLevelScores: hleRunLevelScores,
};

export function hleRunLevelScores(result: RunResult): readonly {
  name: string;
  metrics: Readonly<
    Record<
      string,
      {
        value: number;
      }
    >
  >;
}[] {
  return answerEquivalenceRunLevelScores(result, "hle");
}
