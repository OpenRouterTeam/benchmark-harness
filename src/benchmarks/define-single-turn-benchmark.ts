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
  readonly makeScorerForConfig?: (config: C) => ScorerService;
  readonly makeSolver: (model: ModelService, config: C) => SolverService;
  readonly makeModelLayer?: (
    config: C,
    input: BenchmarkRunInput
  ) => Layer<Model> | undefined;
}

export function defineSingleTurnBenchmark<
  C extends BenchmarkRunConfig & {
    readonly model: string;
    readonly models?: readonly string[];
  },
>(definition: SingleTurnBenchmarkDefinition<C>): Benchmark {
  function makeLayer(
    input: BenchmarkRunInput
  ): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
    const { benchmarkConfig } = input;
    if (!definition.isConfig(benchmarkConfig)) {
      return layerFail(
        new Error(`${definition.id} received mismatched benchmarkConfig`)
      );
    }
    const datasetLayer =
      definition.makeDatasetLayerForConfig !== undefined
        ? definition.makeDatasetLayerForConfig(
            benchmarkConfig,
            input.datasetRetry
          )
        : definition.makeDatasetLayer(input.datasetRetry);
    const modelLayer =
      input.modelLayer ??
      definition.makeModelLayer?.(benchmarkConfig, input) ??
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
    const scorerLayer = layerSucceed(
      Scorer,
      Scorer.of(
        definition.makeScorerForConfig?.(benchmarkConfig) ?? definition.scorer
      )
    );
    return layerMergeAll(datasetLayer, solverLayer, scorerLayer);
  }
  return {
    id: definition.id,
    makeDatasetLayer: definition.makeDatasetLayer,
    temperature: definition.temperature,
    defaultEpochs: definition.defaultEpochs,
    makeLayer,
  };
}
