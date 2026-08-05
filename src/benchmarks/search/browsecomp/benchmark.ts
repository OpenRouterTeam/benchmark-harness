import type { Effect } from "effect/Effect";

import type { Score, Target, TaskState } from "../../../harness/core";
import type { RunResult } from "../../../harness/run";
import type { SolverService } from "../../../harness/solver";
import type { ResponsesService } from "../../../providers/responses-client";
import { BROWSECOMP_META } from "../../benchmark-meta";
import type { Benchmark } from "../../types";
import { makeSearchBenchmarkLayer } from "../core/benchmark";
import { DEEP_RESEARCH_INSTRUCTIONS } from "../core/prompts";
import type { SearchSolverOptions } from "../core/solver";
import {
  answerEquivalenceRunLevelScores,
  answerEquivalenceScorer,
  makeAnswerEquivalenceSolver,
} from "../grading/answer-equivalence-benchmark";
import { makeBrowseCompDatasetLayer } from "./dataset";

export const BROWSECOMP_BENCHMARK_ID = BROWSECOMP_META.id;

const BROWSECOMP_TEMPERATURE = 0;

export function makeBrowseCompSolver(
  responses: ResponsesService,
  options: SearchSolverOptions
): SolverService {
  return makeAnswerEquivalenceSolver(responses, options);
}

export function browseCompScorer(
  state: TaskState,
  _target: Target
): Effect<Score, never> {
  return answerEquivalenceScorer(state, _target);
}

export const BROWSECOMP_BENCHMARK: Benchmark = {
  id: BROWSECOMP_BENCHMARK_ID,
  makeDatasetLayer: makeBrowseCompDatasetLayer,
  temperature: BROWSECOMP_TEMPERATURE,
  defaultEpochs: BROWSECOMP_META.defaultEpochs,
  makeLayer: (input) =>
    makeSearchBenchmarkLayer(input, {
      benchmarkId: BROWSECOMP_BENCHMARK_ID,
      instructions: DEEP_RESEARCH_INSTRUCTIONS,
      makeDatasetLayer: makeBrowseCompDatasetLayer,
      makeSolver: makeBrowseCompSolver,
      scorer: browseCompScorer,
    }),
  runLevelScores: browseCompRunLevelScores,
};

export function browseCompRunLevelScores(result: RunResult): readonly {
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
  return answerEquivalenceRunLevelScores(result, "browsecomp");
}
