import type { HttpClient } from "@effect/platform";
import { fail as effectFail, gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  fail as layerFail,
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";
import { fail as streamFail } from "effect/Stream";

import { DatasetError } from "../../harness/core";
/**
 * `custom_eval` — one generic benchmark whose config carries a declarative
 * {@link EvalSpec}. Customers (and internal users) define an eval as data —
 * dataset + prompt + deterministic scorer — and it runs through the exact
 * same solver/scorer/epochs/results pipeline as first-party benchmarks. The
 * registry stays finite; user evals are rows, not code.
 *
 * Unlike `defineChatBenchmark` benchmarks, the dataset AND scorer both come
 * from the run config, so `makeLayer` is hand-rolled (the tau3/draco pattern).
 */
import { Dataset } from "../../harness/dataset";
import type { GenerateConfig, ModelService } from "../../harness/model";
import { Model } from "../../harness/model";
import type { Scorer as ScorerType } from "../../harness/scorer";
import { Scorer } from "../../harness/scorer";
import type { Solver as SolverType, SolverService } from "../../harness/solver";
import { chain, generate, Solver, systemMessage } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import type { RetryConfig } from "../../runtime/retry";
import type {
  BenchmarkRunConfig,
  CustomEvalBenchmarkConfig,
} from "../benchmark-config";
import { CUSTOM_EVAL_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeCustomEvalDatasetLayer } from "./dataset";
import { makeCustomEvalScorer } from "./scorer";

export const CUSTOM_EVAL_DEFAULT_TEMPERATURE = 0;

export function renderPrompt(
  template: string | undefined,
  input: string
): string {
  if (template === undefined) {
    return input;
  }
  // replaceAll: a template may reference {input} more than once (e.g. quoted
  // and restated); function form so literal `$` sequences in the input survive.
  return template.replaceAll("{input}", () => input);
}

export function customEvalSolver(
  model: ModelService,
  config: CustomEvalBenchmarkConfig
): SolverService {
  const generateConfig: GenerateConfig = {
    temperature: config.temperature ?? CUSTOM_EVAL_DEFAULT_TEMPERATURE,
    ...definedValues({
      maxTokens: config.maxTokens,
      reasoningEffort: config.reasoningEffort,
      timeoutMs: config.timeoutMs,
      sort: config.sort,
      cloudflareVersion: config.cloudflareVersion,
    }),
    ...(config.endpointId !== undefined && { endpointId: config.endpointId }),
  };
  /** Render the prompt template over the sample's input (the last user message). */
  const applyTemplate: SolverService = (state) =>
    generate(
      model,
      generateConfig
    )({
      ...state,
      messages: state.messages.map((message, index) =>
        index === state.messages.length - 1 &&
        typeof message.content === "string"
          ? {
              ...message,
              content: renderPrompt(
                config.spec.promptTemplate,
                message.content
              ),
            }
          : message
      ),
    });
  return config.spec.systemPrompt !== undefined
    ? chain(systemMessage(config.spec.systemPrompt), applyTemplate)
    : applyTemplate;
}

function makeLayer(
  input: BenchmarkRunInput
): Layer<Dataset | SolverType | ScorerType, Error, HttpClient.HttpClient> {
  const config = input.benchmarkConfig;
  if (config.benchmarkId !== "custom_eval") {
    return layerFail(
      new Error("custom_eval received mismatched benchmarkConfig")
    );
  }

  const datasetLayer = makeCustomEvalDatasetLayer(
    config.spec.dataset,
    input.datasetRetry
  );

  const modelLayer =
    input.modelLayer ??
    makeOpenRouterModelLayer({
      model: config.model,
      apiKey: input.apiKey,
      ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
      sessionId: input.sessionId,
      ...(input.modelRetry !== undefined && { retry: input.modelRetry }),
    });

  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* Model;
      return Solver.of(customEvalSolver(model, config));
    })
  ).pipe(layerProvide(modelLayer));

  const scorerLayer = layerSucceed(
    Scorer,
    Scorer.of(makeCustomEvalScorer(config.spec.scorer))
  );

  return layerMergeAll(datasetLayer, solverLayer, scorerLayer);
}

/**
 * The dataset comes from the run config, so the config-free
 * `makeDatasetLayer` (used for registry-wide dataset-size probing of static
 * benchmarks) has nothing real to return. Its size/stream FAIL rather than
 * answering with a placeholder: an earlier stand-in dataset of size 1 made
 * the orchestration chunk every custom eval to its first item and silently
 * score one case. Size probing goes through `makeDatasetLayerForConfig`.
 */
function makeDatasetLayer(_retryConfig?: RetryConfig): Layer<Dataset> {
  const noConfigError = new DatasetError({
    message:
      "custom_eval has no config-free dataset; resolve size via the run config",
  });
  return layerSucceed(
    Dataset,
    Dataset.of({
      stream: () => streamFail(noConfigError),
      size: effectFail(noConfigError),
    })
  );
}

function makeDatasetLayerForConfig(
  config: BenchmarkRunConfig,
  retryConfig?: RetryConfig
): Layer<Dataset> {
  if (config.benchmarkId !== "custom_eval") {
    return makeDatasetLayer(retryConfig);
  }
  return makeCustomEvalDatasetLayer(config.spec.dataset, retryConfig);
}

export const CUSTOM_EVAL_BENCHMARK: Benchmark = {
  id: CUSTOM_EVAL_META.id,
  makeDatasetLayer,
  makeDatasetLayerForConfig,
  makeLayer,
  temperature: CUSTOM_EVAL_DEFAULT_TEMPERATURE,
  defaultEpochs: CUSTOM_EVAL_META.defaultEpochs,
};
