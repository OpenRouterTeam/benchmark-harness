import { gen } from "effect/Effect";
import { HttpClient } from "effect/http";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";
import {
  drop as streamDrop,
  filter as streamFilter,
  runCount,
  take as streamTake,
} from "effect/Stream";

import type { HfDatasetConfig } from "../datasets/huggingface";
import { makeHfDatasetLayer } from "../datasets/huggingface";
import type { Sample } from "../harness/core";
import { Dataset } from "../harness/dataset";
import type { GenerateConfig, ModelService } from "../harness/model";
import { Scorer } from "../harness/scorer";
import type { SolverService } from "../harness/solver";
import { chain, generate, systemMessage } from "../harness/solver";
import { definedValues } from "../internal/guards";
import type { DecisionsTarget } from "../judge/decisions";
import { decisionChoiceProbabilities } from "../judge/decisions";
import type { RetryConfig } from "../runtime/retry";
import type {
  FixedTemperatureInferenceOverride,
  GpqaBenchmarkConfig,
} from "./benchmark-config";
import { GPQA_META } from "./benchmark-meta";
import { defineSingleTurnBenchmark } from "./define-single-turn-benchmark";
import {
  integrityRunLevelScores,
  withIntegrityRubric,
} from "./scorers/integrity/integrity-rubric";
import { INTEGRITY_JUDGE_MODEL } from "./scorers/integrity/integrity-spec";
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
  revision: "c63e9ba02dc3da4c698e2a8485551b35041c3900",
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
    ...definedValues({
      endpointId: opts.endpointId,
    }),
  };
  return chain(
    systemMessage(SIMPLE_EVALS_SYSTEM_MESSAGE),
    generate(model, config)
  );
}

export const gpqaScorer = mcqScorer;

export function makeGpqaScorerLayer(
  config: GpqaBenchmarkConfig,
  target: DecisionsTarget
): Layer<Scorer, never, HttpClient.HttpClient> {
  if (config.integrityRubric !== true) {
    return layerSucceed(Scorer, Scorer.of(gpqaScorer));
  }
  return layerEffect(Scorer)(
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      return Scorer.of(
        withIntegrityRubric(gpqaScorer, (state, question) =>
          decisionChoiceProbabilities(
            client,
            target,
            INTEGRITY_JUDGE_MODEL,
            state,
            question
          )
        )
      );
    })
  );
}

export function makeGpqaDatasetLayer(
  retryConfig?: RetryConfig,
  excludeSampleIds?: readonly string[]
): Layer<Dataset> {
  const hfLayer = makeHfDatasetLayer({
    ...GPQA_DATASET,
    ...definedValues({
      retry: retryConfig,
    }),
  });
  return excludeSampleIds === undefined || excludeSampleIds.length === 0
    ? hfLayer
    : withoutSampleIds(hfLayer, excludeSampleIds);
}

export function withoutSampleIds(
  layer: Layer<Dataset>,
  sampleIds: readonly string[]
): Layer<Dataset> {
  const excluded = new Set(sampleIds);
  return layerEffect(
    Dataset,
    gen(function* () {
      const inner = yield* Dataset;
      const kept = inner
        .stream()
        .pipe(streamFilter((sample) => !excluded.has(sample.id)));
      return Dataset.of({
        size: runCount(kept),
        stream: (opts) => {
          const start = opts?.start ?? 0;
          const dropped = start > 0 ? kept.pipe(streamDrop(start)) : kept;
          return opts?.end === undefined
            ? dropped
            : dropped.pipe(streamTake(Math.max(0, opts.end - start)));
        },
      });
    })
  ).pipe(layerProvide(layer));
}

export const GPQA_BENCHMARK: Benchmark = defineSingleTurnBenchmark({
  id: "gpqa_diamond",
  temperature: GPQA_TEMPERATURE,
  defaultEpochs: GPQA_META.defaultEpochs,
  isConfig: (config): config is GpqaBenchmarkConfig =>
    config.benchmarkId === "gpqa_diamond",
  makeDatasetLayer: makeGpqaDatasetLayer,
  makeDatasetLayerForConfig: (config, retryConfig) =>
    makeGpqaDatasetLayer(retryConfig, config.excludeSampleIds),
  scorer: gpqaScorer,
  makeScorerLayer: (config, input) =>
    makeGpqaScorerLayer(
      config,
      definedValues({ apiKey: input.apiKey, baseUrl: input.baseUrl })
    ),
  runLevelScores: integrityRunLevelScores,
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
          providerOrder: config.providerOrder,
          providerIgnore: config.providerIgnore,
          allowFallbacks: config.allowFallbacks,
          cloudflareVersion: config.cloudflareVersion,
          experimentIds: config.experimentIds,
          costTier: config.costTier,
          costQualityTradeoff: config.costQualityTradeoff,
          switchyardAlgorithm: config.switchyardAlgorithm,
          extraBody: config.extraBody,
        },
      })
    ),
});
