import type { Layer } from "effect/Layer";

import type { Sample } from "../harness/core";
import type { Dataset } from "../harness/dataset";
import type { SampleScore } from "../harness/metric";
import { aggregateScores } from "../harness/metric";
import type { GenerateConfig, ModelService } from "../harness/model";
import type { RunResult } from "../harness/run";
import type { SolverService } from "../harness/solver";
import { generate } from "../harness/solver";
import { definedValues } from "../internal/guards";
import type { RetryConfig } from "../runtime/retry";
import type {
  InferenceOverride,
  MmluProBenchmarkConfig,
} from "./benchmark-config";
import { MMLU_PRO_META } from "./benchmark-meta";
import { defineSingleTurnBenchmark } from "./define-single-turn-benchmark";
import { makeMmluProFewShotDatasetLayer } from "./mmlu-pro-dataset";
import type { MmluProCotExamplesByCategory } from "./mmlu-pro-prompt";
import { buildMmluProPrompt } from "./mmlu-pro-prompt";
import { parseOptions } from "./mmmu-shared";
import { mmluProScorer } from "./scorers/mcq/mmlu-pro-scorer";
import type { Benchmark } from "./types";

export const MMLU_PRO_TEMPERATURE = 0;

const MMLU_PRO_LETTERS = "ABCDEFGHIJ";

function asString(value: unknown, field: string): string {
  if (typeof value !== "string") {
    throw new TypeError(`mmlu_pro record field "${field}" was not a string`);
  }
  return value;
}

export function mmluProRecordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number,
  examplesByCategory: MmluProCotExamplesByCategory
): Sample {
  const question = asString(record["question"], "question");
  const options = parseOptions(record["options"]).filter(
    (option) => option !== "N/A"
  );
  if (options.length === 0 || options.length > MMLU_PRO_LETTERS.length) {
    throw new TypeError(
      'mmlu_pro record field "options" was not a non-empty list of at most 10 strings'
    );
  }
  const answer = asString(record["answer"], "answer");
  if (
    !MMLU_PRO_LETTERS.includes(answer) ||
    answer.length !== 1 ||
    options.length <= MMLU_PRO_LETTERS.indexOf(answer)
  ) {
    throw new TypeError(
      'mmlu_pro record field "answer" was not a valid option letter'
    );
  }
  const category = asString(record["category"], "category");
  const src = asString(record["src"], "src");
  return {
    id: `mmlu_pro-${index}`,
    input: buildMmluProPrompt({
      category,
      cotExamples: examplesByCategory.get(category) ?? [],
      question,
      options,
    }),
    target: { text: answer },
    metadata: { category, src },
  };
}

export const MMLU_PRO_DATASET = {
  dataset: "TIGER-Lab/MMLU-Pro",
  config: "default",
  split: "test",
} as const;

export function makeMmluProDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  return makeMmluProFewShotDatasetLayer(mmluProRecordToSample, retryConfig);
}

export function mmluProSolver(
  model: ModelService,
  opts: {
    readonly endpointId?: string;
    readonly inference: InferenceOverride;
  }
): SolverService {
  const config: GenerateConfig = {
    temperature: MMLU_PRO_TEMPERATURE,
    ...definedValues(opts.inference),
    ...(opts.endpointId !== undefined && { endpointId: opts.endpointId }),
  };
  return generate(model, config);
}

function mmluProRunLevelScores(result: RunResult): readonly {
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
  const categories = new Map<string, SampleScore[]>();
  for (const sampleScore of result.sampleScores) {
    const category = sampleScore.metadata?.["category"];
    if (typeof category !== "string") {
      continue;
    }
    const current = categories.get(category);
    if (current === undefined) {
      categories.set(category, [sampleScore]);
    } else {
      current.push(sampleScore);
    }
  }
  const categoryScores = [...categories.entries()].map(
    ([category, sampleScores]) => {
      const metrics = aggregateScores(sampleScores);
      return {
        name: `mmlu_pro_${category}`,
        metrics: {
          accuracy: { value: metrics.accuracy },
          total_questions: { value: metrics.totalQuestions },
        },
      };
    }
  );
  return [
    {
      name: "mmlu_pro",
      metrics: {
        accuracy: { value: result.metrics.accuracy },
        total_questions: { value: result.metrics.totalQuestions },
      },
    },
    ...categoryScores,
  ];
}

const MMLU_PRO_SINGLE_TURN_BENCHMARK = defineSingleTurnBenchmark({
  id: "mmlu_pro",
  temperature: MMLU_PRO_TEMPERATURE,
  defaultEpochs: MMLU_PRO_META.defaultEpochs,
  isConfig: (config): config is MmluProBenchmarkConfig =>
    config.benchmarkId === "mmlu_pro",
  makeDatasetLayer: makeMmluProDatasetLayer,
  scorer: mmluProScorer,
  makeSolver: (model, config) =>
    mmluProSolver(
      model,
      definedValues({
        endpointId: config.endpointId,
        inference: {
          temperature: config.temperature,
          maxTokens: config.maxTokens,
          reasoningEffort: config.reasoningEffort,
          timeoutMs: config.timeoutMs,
          sort: config.sort,
          providerOnly: config.providerOnly,
          providerIgnore: config.providerIgnore,
          allowFallbacks: config.allowFallbacks,
          cloudflareVersion: config.cloudflareVersion,
          costTier: config.costTier,
          costQualityTradeoff: config.costQualityTradeoff,
        },
      })
    ),
});

export const MMLU_PRO_BENCHMARK: Benchmark = {
  ...MMLU_PRO_SINGLE_TURN_BENCHMARK,
  runLevelScores: mmluProRunLevelScores,
};
