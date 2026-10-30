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
import { McpAtlasConfigSchema } from "../benchmark-config";
import { MCP_ATLAS_META } from "../benchmark-meta";
import { makeMcpSandbox } from "../mcp-sandbox/sandbox";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { makeMcpAtlasDatasetLayer } from "./dataset";
import { MCP_ATLAS_SANDBOX_TOKEN_ENV } from "./schema";
import {
  makeMcpAtlasScorer,
  mcpAtlasJudgeStage,
  mcpAtlasRunLevelScores,
} from "./scorer";
import type { McpAtlasSolverOptions } from "./solver";
import { MCP_ATLAS_TEMPERATURE, mcpAtlasSolver } from "./solver";

export const MCP_ATLAS_BENCHMARK_ID = MCP_ATLAS_META.id;

function readSandboxToken(): string | undefined {
  const token = process.env[MCP_ATLAS_SANDBOX_TOKEN_ENV];
  return token === undefined || token.trim() === "" ? undefined : token;
}

function makeMcpAtlasDatasetLayerForConfig(
  benchmarkConfig: BenchmarkRunConfig,
  retryConfig?: RetryConfig
): Layer<Dataset, Error> {
  const parsed = parseSchema(McpAtlasConfigSchema, benchmarkConfig);
  if (Result.isFailure(parsed)) {
    return layerFail(
      new Error(
        `mcp_atlas received invalid benchmarkConfig: ${parsed.failure.message}`
      )
    );
  }
  return makeMcpAtlasDatasetLayer(
    retryConfig,
    definedValues({ serverAllowlist: parsed.success.serverAllowlist })
  );
}

function makeMcpAtlasLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const configParsed = parseSchema(McpAtlasConfigSchema, input.benchmarkConfig);
  if (Result.isFailure(configParsed)) {
    return layerFail(
      new Error(
        `mcp_atlas received invalid benchmarkConfig: ${configParsed.failure.message}`
      )
    );
  }
  const config = configParsed.success;
  const sandboxToken = readSandboxToken();
  if (sandboxToken === undefined) {
    return layerFail(
      new Error(
        `mcp_atlas requires ${MCP_ATLAS_SANDBOX_TOKEN_ENV} to reach the MCP-Atlas sandbox at ${config.sandboxUrl}`
      )
    );
  }
  const solverOpts: McpAtlasSolverOptions = definedValues({
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
  const datasetLayer = makeMcpAtlasDatasetLayer(
    input.datasetRetry,
    definedValues({ serverAllowlist: config.serverAllowlist })
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
        label: "MCP-Atlas sandbox",
        url: config.sandboxUrl,
        token: sandboxToken,
      });
      const tools = yield* sandbox.listTools;
      const agent = mcpAtlasSolver({ model, sandbox, tools, opts: solverOpts });
      const judge = mcpAtlasJudgeStage(
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
  const scorerLayer = layerSucceed(
    Scorer,
    Scorer.of(makeMcpAtlasScorer(config.passThreshold))
  );
  return layerMergeAll(
    datasetLayer,
    solverLayer.pipe(layerProvide(layerMergeAll(modelLayer, responsesLayer))),
    scorerLayer
  );
}

export const MCP_ATLAS_BENCHMARK: Benchmark = {
  id: MCP_ATLAS_BENCHMARK_ID,
  makeDatasetLayer: (retryConfig) => makeMcpAtlasDatasetLayer(retryConfig),
  makeDatasetLayerForConfig: makeMcpAtlasDatasetLayerForConfig,
  temperature: MCP_ATLAS_TEMPERATURE,
  defaultEpochs: MCP_ATLAS_META.defaultEpochs,
  degradeSolverErrors: true,
  makeLayer: makeMcpAtlasLayer,
  runLevelScores: mcpAtlasRunLevelScores,
};
