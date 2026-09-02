import { HttpClient } from "@effect/platform";
import { gen, makeSemaphore } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  fail as layerFail,
  effect as layerEffect,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";

import type { HfDatasetConfig } from "../../datasets/huggingface";
import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { Sample } from "../../harness/core";
import type { Dataset } from "../../harness/dataset";
import { Model } from "../../harness/model";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import {
  makeResponsesModelLayer,
  ResponsesModel,
} from "../../providers/responses-model";
import type { RetryConfig } from "../../runtime/retry";
import { TAU_BENCH_AIRLINE_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { airlineScorer } from "./scorer";
import { airlineSolver } from "./solver";
import type { SolverOpts, Tau2Task } from "./types";
import { renderUserInstructions, Tau2TaskSchema } from "./types";

export const TAU_BENCH_AIRLINE_TEMPERATURE = TAU_BENCH_AIRLINE_META.temperature;

export const TAU_BENCH_AIRLINE_ID = TAU_BENCH_AIRLINE_META.id;

export function airlineRecordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number
): Sample {
  const rawTaskJson = record["task_json"];
  if (typeof rawTaskJson !== "string") {
    throw new TypeError(
      `tau-bench-verified-airline row ${index} missing string 'task_json' column`
    );
  }
  const json = Either.try((): unknown => JSON.parse(rawTaskJson));
  if (Either.isLeft(json)) {
    throw new Error(
      `Failed to JSON.parse task_json at index ${index}: ${String(json.left)}`
    );
  }
  const parsed = parseSchema(Tau2TaskSchema, json.right);
  if (Either.isLeft(parsed)) {
    throw new Error(
      `Failed to parse tau-bench-verified-airline task at index ${index}: ${parsed.left.message}`
    );
  }
  const task: Tau2Task = parsed.right;
  return {
    id: `${TAU_BENCH_AIRLINE_ID}-${task.id}`,
    input: renderUserInstructions(task.user_scenario.instructions),
    target: { text: "" },
    metadata: { task },
  };
}

export const TAU_BENCH_AIRLINE_DATASET = {
  dataset: "abhinavpola/tau2-bench-verified-airline",
  config: "tasks",
  split: "test",
  recordToSample: airlineRecordToSample,
} as const satisfies Omit<HfDatasetConfig, "pageSize">;

export function makeAirlineDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  return makeHfDatasetLayer({
    ...TAU_BENCH_AIRLINE_DATASET,
    ...definedValues({
      retry: retryConfig,
    }),
  });
}

function makeAirlineLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== "tau_bench_verified_airline") {
    return layerFail(
      new Error(
        "tau_bench_verified_airline received mismatched benchmarkConfig"
      )
    );
  }
  const solverOpts: SolverOpts = definedValues({
    endpointId: benchmarkConfig.endpointId,
    userModelConfig: definedValues({
      apiKey: input.apiKey,
      model: benchmarkConfig.userModel,
      baseUrl: input.baseUrl,
      sessionId: input.sessionId,
      reasoningEffort: benchmarkConfig.userReasoningEffort,
    }),
    inference: {
      maxTokens: benchmarkConfig.maxTokens,
      reasoningEffort: benchmarkConfig.reasoningEffort,
      timeoutMs: benchmarkConfig.timeoutMs,
      sort: benchmarkConfig.sort,
      providerOnly: benchmarkConfig.providerOnly,
      providerIgnore: benchmarkConfig.providerIgnore,
      allowFallbacks: benchmarkConfig.allowFallbacks,
      cloudflareVersion: benchmarkConfig.cloudflareVersion,
      costTier: benchmarkConfig.costTier,
      costQualityTradeoff: benchmarkConfig.costQualityTradeoff,
      pinModel: benchmarkConfig.pinModel,
    },
  });
  const datasetLayer = makeAirlineDatasetLayer(input.datasetRetry);
  const modelLayer =
    input.modelLayer ??
    makeOpenRouterModelLayer(
      definedValues({
        model: benchmarkConfig.model,
        apiKey: input.apiKey,
        baseUrl: input.baseUrl,
        sessionId: input.sessionId,
        retry: input.modelRetry,
        traceHeaders: input.traceHeaders,
      })
    );
  const userModelLayer = makeResponsesModelLayer(
    definedValues({
      model: benchmarkConfig.userModel,
      apiKey: input.apiKey,
      baseUrl: input.baseUrl,
      sessionId: input.sessionId,
      traceHeaders: input.traceHeaders,
    })
  );
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* Model;
      const userModel = yield* ResponsesModel;
      const client = yield* HttpClient.HttpClient;
      const dataFetchLock = yield* makeSemaphore(1);
      return Solver.of(
        airlineSolver({
          model,
          userModel,
          client,
          dataFetchLock,
          opts: solverOpts,
        })
      );
    })
  );
  const scorerLayer = layerSucceed(Scorer, Scorer.of(airlineScorer));
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(layerMergeAll(modelLayer, userModelLayer))),
    scorerLayer
  );
}

export const TAU_BENCH_AIRLINE_BENCHMARK: Benchmark = {
  id: TAU_BENCH_AIRLINE_ID,
  makeDatasetLayer: makeAirlineDatasetLayer,
  temperature: TAU_BENCH_AIRLINE_TEMPERATURE,
  defaultEpochs: TAU_BENCH_AIRLINE_META.defaultEpochs,
  userModel: TAU_BENCH_AIRLINE_META.userModel,
  makeLayer: makeAirlineLayer,
};
