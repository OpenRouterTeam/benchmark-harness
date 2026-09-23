import { HttpClient } from "@effect/platform";
import { gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  fail as layerFail,
  effect as layerEffect,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";

import type { Dataset } from "../harness/dataset";
import type { ModelService } from "../harness/model";
import { Model } from "../harness/model";
import type { ScorerService } from "../harness/scorer";
import { Scorer } from "../harness/scorer";
import type { SolverService } from "../harness/solver";
import { Solver } from "../harness/solver";
import { definedValues } from "../internal/guards";
import { makeOpenRouterModelLayer } from "../providers/openrouter-model";
import type { RetryConfig } from "../runtime/retry";
import type { BenchmarkRunConfig } from "./benchmark-config";
import type { Benchmark, BenchmarkRunInput } from "./types";

export interface SingleTurnBenchmarkDefinition<
  C extends BenchmarkRunConfig & {
    readonly model: string;
    readonly models?: readonly string[];
  },
> {
  readonly id: C["benchmarkId"];
  readonly temperature: number;
  readonly defaultEpochs: number;
  readonly isConfig: (config: BenchmarkRunConfig) => config is C;
  readonly makeDatasetLayer: (retryConfig?: RetryConfig) => Layer<Dataset>;
  readonly makeDatasetLayerForConfig?: (
    config: C,
    retryConfig?: RetryConfig
  ) => Layer<Dataset>;
  readonly scorer: ScorerService;
  readonly makeSolver: (model: ModelService, config: C) => SolverService;
}

export function defineSingleTurnBenchmark<
  C extends BenchmarkRunConfig & {
    readonly model: string;
    readonly models?: readonly string[];
  },
>(definition: SingleTurnBenchmarkDefinition<C>): Benchmark {
  function selectDatasetLayer(
    config: C,
    retryConfig?: RetryConfig
  ): Layer<Dataset> {
    return (
      definition.makeDatasetLayerForConfig?.(config, retryConfig) ??
      definition.makeDatasetLayer(retryConfig)
    );
  }

  function makeDatasetLayerForConfig(
    config: BenchmarkRunConfig,
    retryConfig?: RetryConfig
  ): Layer<Dataset, Error> {
    if (!definition.isConfig(config)) {
      return layerFail(
        new Error(`${definition.id} received mismatched benchmarkConfig`)
      );
    }
    return selectDatasetLayer(config, retryConfig);
  }

  function makeLayer(
    input: BenchmarkRunInput
  ): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
    const { benchmarkConfig } = input;
    if (!definition.isConfig(benchmarkConfig)) {
      return layerFail(
        new Error(`${definition.id} received mismatched benchmarkConfig`)
      );
    }
    const datasetLayer = selectDatasetLayer(
      benchmarkConfig,
      input.datasetRetry
    );
    const modelLayer =
      input.modelLayer ??
      makeOpenRouterModelLayer(
        definedValues({
          model: benchmarkConfig.model,
          models: benchmarkConfig.models,
          apiKey: input.apiKey,
          baseUrl: input.baseUrl,
          sessionId: input.sessionId,
          retry: input.modelRetry,
          traceHeaders: input.traceHeaders,
        })
      );
    const solverLayer = layerEffect(Solver)(
      gen(function* () {
        const model = yield* Model;
        return Solver.of(definition.makeSolver(model, benchmarkConfig));
      })
    ).pipe(layerProvide(modelLayer));
    const scorerLayer = layerSucceed(Scorer, Scorer.of(definition.scorer));
    return layerMergeAll(datasetLayer, solverLayer, scorerLayer);
  }
  return {
    id: definition.id,
    makeDatasetLayer: definition.makeDatasetLayer,
    makeDatasetLayerForConfig,
    temperature: definition.temperature,
    defaultEpochs: definition.defaultEpochs,
    makeLayer,
  };
}
