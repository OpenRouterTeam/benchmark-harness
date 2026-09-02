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
import { Tau3BenchBankingConfigSchema } from "../benchmark-config";
import { TAU3_BENCH_BANKING_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeBankingDatasetLayer } from "./dataset";
import { bankingScorer } from "./scorer";
import { bankingSolver } from "./solver";
import type { SolverOpts } from "./types";

export const TAU3_BENCH_BANKING_TEMPERATURE = 0;

export const TAU3_BENCH_BANKING_ID = TAU3_BENCH_BANKING_META.id;

function makeBankingLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  const configParsed = parseSchema(
    Tau3BenchBankingConfigSchema,
    benchmarkConfig
  );
  if (Either.isLeft(configParsed)) {
    return layerFail(
      new Error(
        `tau3_bench_banking received invalid benchmarkConfig: ${configParsed.left.message}`
      )
    );
  }
  const config = configParsed.right;
  const solverOpts: SolverOpts = definedValues({
    endpointId: config.endpointId,
    userModelConfig: definedValues({
      apiKey: input.apiKey,
      model: config.userModel,
      baseUrl: input.baseUrl,
      sessionId: input.sessionId,
      userReasoningEffort: config.userReasoningEffort,
    }),
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
      pinModel: config.pinModel,
    },
    retrievalConfig: config.retrievalConfig,
  });
  const datasetLayer = makeBankingDatasetLayer(input.datasetRetry);
  const modelLayer =
    input.modelLayer ??
    makeOpenRouterModelLayer(
      definedValues({
        model: config.model,
        apiKey: input.apiKey,
        baseUrl: input.baseUrl,
        sessionId: input.sessionId,
        retry: input.modelRetry,
        traceHeaders: input.traceHeaders,
      })
    );
  const userModelLayer = makeResponsesModelLayer(
    definedValues({
      model: config.userModel,
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
        bankingSolver({
          model,
          userModel,
          client,
          dataFetchLock,
          opts: solverOpts,
        })
      );
    })
  );
  const scorerLayer = layerSucceed(Scorer, Scorer.of(bankingScorer));
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(layerMergeAll(modelLayer, userModelLayer))),
    scorerLayer
  );
}

export const TAU3_BENCH_BANKING_BENCHMARK: Benchmark = {
  id: TAU3_BENCH_BANKING_ID,
  makeDatasetLayer: makeBankingDatasetLayer,
  temperature: TAU3_BENCH_BANKING_TEMPERATURE,
  defaultEpochs: TAU3_BENCH_BANKING_META.defaultEpochs,
  userModel: TAU3_BENCH_BANKING_META.userModel,
  makeLayer: makeBankingLayer,
};
