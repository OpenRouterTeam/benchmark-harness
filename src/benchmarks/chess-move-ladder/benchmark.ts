import { fail, succeed } from "effect/Effect";
import type { Layer } from "effect/Layer";

import type { HfDatasetConfig } from "../../datasets/huggingface";
import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { Sample } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { Dataset } from "../../harness/dataset";
import type { GenerateConfig, ModelService } from "../../harness/model";
import { Model } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { chain, generate } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import {
  makeSystemOneModelLayer,
  systemOneRequestMessage,
} from "../../providers/systemone-model";
import type { RetryConfig } from "../../runtime/retry";
import type {
  ChessMoveLadderBenchmarkConfig,
  InferenceOverride,
} from "../benchmark-config";
import { CHESS_MOVE_LADDER_META } from "../benchmark-meta";
import { defineSingleTurnBenchmark } from "../define-single-turn-benchmark";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { parseChessItem, puzzleRecordToItem } from "./item";
import { renderChatPrompt, renderSystemOneRequest } from "./prompt";
import type { ChessCondition, ModelInterface } from "./schema";
import {
  CHESS_MOVE_LADDER_ID,
  DEFAULT_CANDIDATE_COUNT,
  conditionFromOptions,
  resolveModelInterface,
} from "./schema";
import { makeChessScorer } from "./scorer";

export const CHESS_MOVE_LADDER_TEMPERATURE = 0;

export function chessRecordToSample(
  record: Readonly<Record<string, unknown>>,
  candidateCount: number
): Sample {
  const item = puzzleRecordToItem(record, candidateCount);
  return {
    id: `${CHESS_MOVE_LADDER_ID}-${item.puzzleId}`,
    input: item.fen,
    target: { text: item.solutionLetter },
    metadata: { item },
  };
}

export const CHESS_MOVE_LADDER_DATASET = {
  dataset: "Lichess/chess-puzzles",
  config: "default",
  split: "train",
} as const satisfies Omit<HfDatasetConfig, "pageSize" | "recordToSample">;

export function makeChessDatasetLayer(
  candidateCount: number,
  retryConfig?: RetryConfig
): Layer<Dataset> {
  return makeHfDatasetLayer({
    ...CHESS_MOVE_LADDER_DATASET,
    recordToSample: (record) => chessRecordToSample(record, candidateCount),
    ...definedValues({ retry: retryConfig }),
  });
}

function renderInput(
  condition: ChessCondition,
  modelInterface: ModelInterface
): SolverService {
  return (state) => {
    const item = parseChessItem(state.sample.metadata);
    if (Either.isLeft(item)) {
      return fail(
        new SolverError({
          message: `chess_move_ladder sample ${state.sample.id} has invalid metadata: ${item.left}`,
        })
      );
    }
    const message =
      modelInterface === "systemone"
        ? systemOneRequestMessage(renderSystemOneRequest(item.right, condition))
        : {
            role: MessageRole.User,
            content: renderChatPrompt(item.right, condition),
          };
    return succeed({
      ...state,
      messages: [message],
      sample: {
        ...state.sample,
        metadata: {
          ...state.sample.metadata,
          condition,
          modelInterface,
        },
      },
    });
  };
}

export function chessSolver(
  model: ModelService,
  opts: {
    readonly condition: ChessCondition;
    readonly modelInterface: ModelInterface;
    readonly endpointId?: string;
    readonly inference: InferenceOverride;
  }
): SolverService {
  const config: GenerateConfig = {
    temperature: CHESS_MOVE_LADDER_TEMPERATURE,
    ...definedValues(opts.inference),
    ...definedValues({ endpointId: opts.endpointId }),
  };
  return chain(
    renderInput(opts.condition, opts.modelInterface),
    generate(model, config)
  );
}

function interfaceFor(config: ChessMoveLadderBenchmarkConfig): ModelInterface {
  return resolveModelInterface(config.model, config.interface);
}

function makeChessModelLayer(
  config: ChessMoveLadderBenchmarkConfig,
  input: BenchmarkRunInput
): Layer<Model> | undefined {
  if (interfaceFor(config) !== "systemone") {
    return undefined;
  }
  return makeSystemOneModelLayer(
    definedValues({
      model: config.model,
      apiKey: input.apiKey,
      baseUrl: input.baseUrl,
      sessionId: input.sessionId,
      retry: input.modelRetry,
      traceHeaders: input.traceHeaders,
    })
  );
}

export const CHESS_MOVE_LADDER_BENCHMARK: Benchmark = defineSingleTurnBenchmark(
  {
    id: CHESS_MOVE_LADDER_ID,
    temperature: CHESS_MOVE_LADDER_TEMPERATURE,
    defaultEpochs: CHESS_MOVE_LADDER_META.defaultEpochs,
    isConfig: (config): config is ChessMoveLadderBenchmarkConfig =>
      config.benchmarkId === CHESS_MOVE_LADDER_ID,
    makeDatasetLayer: (retryConfig) =>
      makeChessDatasetLayer(DEFAULT_CANDIDATE_COUNT, retryConfig),
    makeDatasetLayerForConfig: (config, retryConfig) =>
      makeChessDatasetLayer(config.candidateCount, retryConfig),
    scorer: makeChessScorer("chat"),
    makeScorerForConfig: (config) => makeChessScorer(interfaceFor(config)),
    makeModelLayer: makeChessModelLayer,
    makeSolver: (model, config) =>
      chessSolver(
        model,
        definedValues({
          condition: conditionFromOptions(config),
          modelInterface: interfaceFor(config),
          endpointId: config.endpointId,
          inference: {
            temperature: config.temperature,
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
  }
);
