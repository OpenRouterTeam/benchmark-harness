import { HttpClient } from "@effect/platform";
import { gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  fail as layerFail,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";

import type { Dataset } from "../../harness/dataset";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import type {
  BenchmarkRunConfig,
  ToolCallSchemaFuzzBenchmarkConfig,
} from "../benchmark-config";
import { TOOLCALL_SCHEMA_FUZZ_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { caseLookup, toolCallSchemaFuzzCases } from "./cases";
import { makeToolCallSchemaFuzzDatasetLayer } from "./dataset";
import { ToolCallSchemaFuzzGenerator } from "./options";
import { makeToolCallSchemaFuzzScorer } from "./scorer";
import { toolCallSchemaFuzzSolver } from "./solver";

function isToolCallSchemaFuzzConfig(
  config: BenchmarkRunConfig
): config is ToolCallSchemaFuzzBenchmarkConfig {
  return config.benchmarkId === TOOLCALL_SCHEMA_FUZZ_META.id;
}

function datasetLayerFor(
  generator: ToolCallSchemaFuzzGenerator
): Layer<Dataset, Error> {
  const cases = toolCallSchemaFuzzCases({ generator });
  if (cases.length === 0) {
    return layerFail(
      new Error(
        `${TOOLCALL_SCHEMA_FUZZ_META.id} generator "${generator}" has no cases`
      )
    );
  }
  return makeToolCallSchemaFuzzDatasetLayer(cases);
}

function makeLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const config = input.benchmarkConfig;
  if (!isToolCallSchemaFuzzConfig(config)) {
    return layerFail(
      new Error(
        `${TOOLCALL_SCHEMA_FUZZ_META.id} received mismatched benchmarkConfig`
      )
    );
  }
  const lookup = caseLookup(
    toolCallSchemaFuzzCases({ generator: config.generator })
  );
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      return Solver.of(
        toolCallSchemaFuzzSolver(client, {
          cases: lookup,
          options: {
            model: config.model,
            apiKey: input.apiKey,
            reasoningEffort: config.reasoningEffort,
            sessionId: input.sessionId,
            ...definedValues({
              baseUrl: input.baseUrl,
              traceHeaders: input.traceHeaders,
              retry: input.modelRetry,
              endpointId: config.endpointId,
              maxTokens: config.maxTokens,
              timeoutMs: config.timeoutMs,
              sort: config.sort,
              cloudflareVersion: config.cloudflareVersion,
              experimentIds: config.experimentIds,
              costTier: config.costTier,
              costQualityTradeoff: config.costQualityTradeoff,
              pinModel: config.pinModel,
              providerOnly: config.providerOnly,
              providerIgnore: config.providerIgnore,
              allowFallbacks: config.allowFallbacks,
              switchyardAlgorithm: config.switchyardAlgorithm,
              models: config.models,
            }),
          },
        })
      );
    })
  );
  return layerMergeAll(
    datasetLayerFor(config.generator),
    solverLayer,
    layerSucceed(Scorer, Scorer.of(makeToolCallSchemaFuzzScorer(lookup)))
  );
}

export const TOOLCALL_SCHEMA_FUZZ_BENCHMARK: Benchmark = {
  id: TOOLCALL_SCHEMA_FUZZ_META.id,
  temperature: TOOLCALL_SCHEMA_FUZZ_META.temperature,
  defaultEpochs: TOOLCALL_SCHEMA_FUZZ_META.defaultEpochs,
  makeDatasetLayer: () =>
    makeToolCallSchemaFuzzDatasetLayer(
      toolCallSchemaFuzzCases({
        generator: ToolCallSchemaFuzzGenerator.Realistic,
      })
    ),
  makeDatasetLayerForConfig: (config) =>
    isToolCallSchemaFuzzConfig(config)
      ? datasetLayerFor(config.generator)
      : layerFail(
          new Error(
            `${TOOLCALL_SCHEMA_FUZZ_META.id} received mismatched benchmarkConfig`
          )
        ),
  makeLayer,
};
