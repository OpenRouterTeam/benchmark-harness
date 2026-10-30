import { flatMap, gen } from "effect/Effect";
import { HttpClient } from "effect/http";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";
import * as Result from "effect/Result";

import type { Dataset } from "../../harness/dataset";
import { Model } from "../../harness/model";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { layerFail } from "../../internal/layer";
import { parseSchema } from "../../internal/zod";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import {
  makeResponsesLayer,
  Responses,
} from "../../providers/responses-client";
import type { RetryConfig } from "../../runtime/retry";
import type { BenchmarkRunConfig } from "../benchmark-config";
import { LiveMcpBenchConfigSchema } from "../benchmark-config";
import { LIVE_MCP_BENCH_META } from "../benchmark-meta";
import { makeMcpSandbox } from "../mcp-sandbox/sandbox";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeLiveMcpBenchDatasetLayer } from "./dataset";
import { LIVE_MCP_BENCH_SANDBOX_TOKEN_ENV } from "./schema";
import {
  liveMcpBenchJudgeStage,
  liveMcpBenchRunLevelScores,
  makeLiveMcpBenchScorer,
} from "./scorer";
import type { LiveMcpBenchSolverOptions } from "./solver";
import { LIVE_MCP_BENCH_TEMPERATURE, liveMcpBenchSolver } from "./solver";

export const LIVE_MCP_BENCH_BENCHMARK_ID = LIVE_MCP_BENCH_META.id;

function readSandboxToken(): string | undefined {
  const token = process.env[LIVE_MCP_BENCH_SANDBOX_TOKEN_ENV];
  return token === undefined || token.trim() === "" ? undefined : token;
}

function makeLiveMcpBenchDatasetLayerForConfig(
  benchmarkConfig: BenchmarkRunConfig,
  retryConfig?: RetryConfig
): Layer<Dataset, Error> {
  const parsed = parseSchema(LiveMcpBenchConfigSchema, benchmarkConfig);
  if (Result.isFailure(parsed)) {
    return layerFail(
      new Error(
        `live_mcp_bench received invalid benchmarkConfig: ${parsed.failure.message}`
      )
    );
  }
  return makeLiveMcpBenchDatasetLayer(
    retryConfig,
    definedValues({ categories: parsed.success.categories })
  );
}

function makeLiveMcpBenchLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const configParsed = parseSchema(
    LiveMcpBenchConfigSchema,
    input.benchmarkConfig
  );
  if (Result.isFailure(configParsed)) {
    return layerFail(
      new Error(
        `live_mcp_bench received invalid benchmarkConfig: ${configParsed.failure.message}`
      )
    );
  }
  const config = configParsed.success;
  const sandboxToken = readSandboxToken();
  if (sandboxToken === undefined) {
    return layerFail(
      new Error(
        `live_mcp_bench requires ${LIVE_MCP_BENCH_SANDBOX_TOKEN_ENV} to reach the LiveMCPBench sandbox at ${config.sandboxUrl}`
      )
    );
  }
  const solverOpts: LiveMcpBenchSolverOptions = definedValues({
    toolScope: config.toolScope,
    maxTurns: config.maxTurns,
    maxToolCalls: config.maxToolCalls,
    toolOutputCap: config.toolOutputCap,
    endpointId: config.endpointId,
    inference: {
      maxTokens: config.maxTokens,
      reasoningEffort: config.reasoningEffort,
      timeoutMs: config.timeoutMs,
      sort: config.sort,
      providerOrder: config.providerOrder,
      providerOnly: config.providerOnly,
      providerIgnore: config.providerIgnore,
      allowFallbacks: config.allowFallbacks,
      cloudflareVersion: config.cloudflareVersion,
      experimentIds: config.experimentIds,
      costTier: config.costTier,
      costQualityTradeoff: config.costQualityTradeoff,
      pinModel: config.pinModel,
      switchyardAlgorithm: config.switchyardAlgorithm,
      extraBody: config.extraBody,
    },
  });
  const datasetLayer = makeLiveMcpBenchDatasetLayer(
    input.datasetRetry,
    definedValues({ categories: config.categories })
  );
  const modelLayer =
    input.modelLayer ??
    makeOpenRouterModelLayer(
      definedValues({
        model: config.model,
        models: config.models,
        apiKey: input.apiKey,
        baseUrl: input.baseUrl,
        sessionId: input.sessionId,
        retry: input.modelRetry,
        traceHeaders: input.traceHeaders,
      })
    );
  const responsesLayer = makeResponsesLayer(
    definedValues({
      apiKey: input.apiKey,
      baseUrl: input.baseUrl,
      sessionId: input.sessionId,
      traceHeaders: input.traceHeaders,
    })
  );
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const model = yield* Model;
      const responses = yield* Responses;
      const client = yield* HttpClient.HttpClient;
      const sandbox = makeMcpSandbox(client, {
        label: "LiveMCPBench sandbox",
        url: config.sandboxUrl,
        token: sandboxToken,
      });
      const tools = yield* sandbox.listTools;
      const agent = liveMcpBenchSolver({
        model,
        sandbox,
        tools,
        opts: solverOpts,
      });
      const judge = liveMcpBenchJudgeStage(
        responses,
        definedValues({
          judgeModel: config.judgeModel,
          reasoningEffort: config.judgeReasoningEffort,
          temperature: 0,
          retry: input.modelRetry,
        })
      );
      return Solver.of((state) => agent(state).pipe(flatMap(judge)));
    })
  );
  const scorerLayer = layerSucceed(Scorer, Scorer.of(makeLiveMcpBenchScorer()));
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(layerMergeAll(modelLayer, responsesLayer))),
    scorerLayer
  );
}

export const LIVE_MCP_BENCH_BENCHMARK: Benchmark = {
  id: LIVE_MCP_BENCH_BENCHMARK_ID,
  makeDatasetLayer: (retryConfig) => makeLiveMcpBenchDatasetLayer(retryConfig),
  makeDatasetLayerForConfig: makeLiveMcpBenchDatasetLayerForConfig,
  temperature: LIVE_MCP_BENCH_TEMPERATURE,
  defaultEpochs: LIVE_MCP_BENCH_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeLiveMcpBenchLayer,
  runLevelScores: liveMcpBenchRunLevelScores,
};
