import type { Layer } from "effect/Layer";

import type { HfDatasetConfig } from "../datasets/huggingface";
import { makeHfDatasetLayer } from "../datasets/huggingface";
import type { Sample } from "../harness/core";
import type { Dataset } from "../harness/dataset";
import type { GenerateConfig, ModelService } from "../harness/model";
import type { SolverService } from "../harness/solver";
import { chain, generate, systemMessage } from "../harness/solver";
import { definedValues } from "../internal/guards";
import type { RetryConfig } from "../runtime/retry";
import type {
  FixedTemperatureInferenceOverride,
  GpqaBenchmarkConfig,
} from "./benchmark-config";
import { GPQA_META } from "./benchmark-meta";
import { defineSingleTurnBenchmark } from "./define-single-turn-benchmark";
import { mcqScorer } from "./scorers/mcq/scorer";
import { seededPermutation } from "./scorers/mcq/shuffle";
import type { Benchmark } from "./types";

export const SIMPLE_EVALS_SYSTEM_MESSAGE = "You are a helpful assistant.";

export const MULTIPLE_CHOICE_PROMPT_TEMPLATE = `Answer the following multiple choice question. The last line of your response should be of the following format: 'Answer: $LETTER' (without quotes) where LETTER is one of ABCD.

{prompt}

A) {option_a}
B) {option_b}
C) {option_c}
D) {option_d}`;

export const GPQA_TEMPERATURE = GPQA_META.temperature;

const GPQA_OPTION_FIELDS = [
  "Correct Answer",
  "Incorrect Answer 1",
  "Incorrect Answer 2",
  "Incorrect Answer 3",
] as const;

const CORRECT_ANSWER_ORIGINAL_INDEX = 0;

function asString(value: unknown, field: string): string {
  if (typeof value !== "string") {
    throw new TypeError(`gpqa record field "${field}" was not a string`);
  }
  return value;
}

export function gpqaRecordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number
): Sample {
  const question = asString(record["Question"], "Question");
  const optionsByOriginalIndex = GPQA_OPTION_FIELDS.map((field) =>
    asString(record[field], field)
  );
  const permutation = seededPermutation(GPQA_OPTION_FIELDS.length, index);
  const shuffled = permutation.map(
    (originalIdx) => optionsByOriginalIndex[originalIdx]!
  );
  const correctPosition = permutation.indexOf(CORRECT_ANSWER_ORIGINAL_INDEX);
  const correctLetter = "ABCD"[correctPosition]!;
  const fill = (template: string, token: string, value: string): string =>
    template.replace(token, () => value);
  const tokens: [string, string][] = [
    ["{prompt}", question],
    ["{option_a}", shuffled[0]!],
    ["{option_b}", shuffled[1]!],
    ["{option_c}", shuffled[2]!],
    ["{option_d}", shuffled[3]!],
  ];
  let input = MULTIPLE_CHOICE_PROMPT_TEMPLATE;
  for (const [token, value] of tokens) {
    input = fill(input, token, value);
  }
  return {
    id: `gpqa_diamond-${index}`,
    input,
    target: { text: correctLetter },
    metadata: { subdomain: record["Subdomain"] },
  };
}

export const GPQA_DATASET = {
  dataset: "nmayorga7/gpqa_diamond",
  config: "default",
  split: "train",
  recordToSample: gpqaRecordToSample,
} as const satisfies Omit<HfDatasetConfig, "pageSize">;

export function gpqaSolver(
  model: ModelService,
  opts: {
    readonly endpointId?: string;
    readonly inference: FixedTemperatureInferenceOverride;
  }
): SolverService {
  const config: GenerateConfig = {
    temperature: GPQA_TEMPERATURE,
    ...definedValues(opts.inference),
    ...(opts.endpointId !== undefined && { endpointId: opts.endpointId }),
  };
  return chain(
    systemMessage(SIMPLE_EVALS_SYSTEM_MESSAGE),
    generate(model, config)
  );
}

export const gpqaScorer = mcqScorer;

export function makeGpqaDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  return makeHfDatasetLayer({
    ...GPQA_DATASET,
    ...(retryConfig !== undefined && { retry: retryConfig }),
  });
}

export const GPQA_BENCHMARK: Benchmark = defineSingleTurnBenchmark({
  id: "gpqa_diamond",
  temperature: GPQA_TEMPERATURE,
  defaultEpochs: GPQA_META.defaultEpochs,
  isConfig: (config): config is GpqaBenchmarkConfig =>
    config.benchmarkId === "gpqa_diamond",
  makeDatasetLayer: makeGpqaDatasetLayer,
  scorer: gpqaScorer,
  makeSolver: (model, config) =>
    gpqaSolver(
      model,
      definedValues({
        endpointId: config.endpointId,
        inference: {
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
