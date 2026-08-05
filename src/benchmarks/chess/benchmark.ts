import { HttpClient } from "@effect/platform";
import { gen, succeed } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  fail as layerFail,
  effect as layerEffect,
  provide as layerProvide,
  mergeAll as layerMergeAll,
  succeed as layerSucceed,
} from "effect/Layer";
import type { Stream } from "effect/Stream";
import { fromIterable } from "effect/Stream";

/**
 * Chess benchmark — full games against a real UCI engine (Stockfish).
 *
 * Measures a model's ability to RETAIN THE POSITION across a complete
 * back-and-forth game in SAN notation, with per-ply Stockfish evaluation and
 * fully deterministic scoring (no judge model anywhere). Faithful port of the
 * byo-benchmark chess bench: same protocol, extraction, adjudication rules,
 * and scoring semantics.
 *
 * Five tasks: full game as White, as Black, with "check <move>" probes, in
 * strict-SAN mode, and a K+Q vs K endgame conversion (mate or fail).
 *
 * Requires a Stockfish binary (`brew install stockfish` /
 * `apt-get install stockfish`, or STOCKFISH_PATH). The solver fails closed —
 * per sample, before any model spend — when the engine is missing.
 *
 * Concurrency: every sample owns its engine pair; nothing is shared between
 * games, so harness-level concurrency is bounded only by process headroom
 * (2 single-threaded engine processes per in-flight game).
 */
import type { Sample } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { Dataset, DatasetStreamOptions } from "../../harness/dataset";
import { Dataset as DatasetTag } from "../../harness/dataset";
import { Model } from "../../harness/model";
import type { RunResult } from "../../harness/run";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import { makeOpenRouterModelLayer } from "../../providers/openrouter-model";
import type { RetryConfig } from "../../runtime/retry";
import { ChessBenchmarkConfigSchema } from "../benchmark-config";
import { CHESS_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import { CHESS_TASK_DEFINITIONS, CHESS_TASKS } from "./game";
import { ChessGameRecordSchema } from "./schema";
import { chessScorer } from "./scorer";
import { chessSolver } from "./solver";

export const CHESS_TEMPERATURE = 0;

//#region Dataset

function chessTaskToSample(taskId: (typeof CHESS_TASKS)[number]): Sample {
  const task = CHESS_TASK_DEFINITIONS[taskId];
  return {
    id: `chess-${taskId}`,
    input: `Full chess game vs Stockfish depth ${task.engineDepth} as ${
      task.modelColor === "w" ? "White" : "Black"
    } (${taskId})`,
    /* State-based scoring: the target is the game outcome, not a string. */
    target: { text: "" },
    metadata: { task },
  };
}

function makeChessDatasetLayer(_retryConfig?: RetryConfig): Layer<Dataset> {
  const samples = CHESS_TASKS.map(chessTaskToSample);
  const stream = (opts?: DatasetStreamOptions): Stream<Sample, never> =>
    fromIterable(samples.slice(opts?.start ?? 0, opts?.end ?? samples.length));
  return layerSucceed(
    DatasetTag,
    DatasetTag.of({ stream, size: succeed(samples.length) })
  );
}

//#endregion

//#region Run-level scores

const CHESS_RUN_METRICS = [
  "points",
  "acpl",
  "illegalAttempts",
  "blunders",
  "gameLengthMoves",
] as const;

/**
 * Aggregate the deterministic per-game quality profile across the run:
 * average points (chess score), ACPL, illegal attempts, blunders, and game
 * length — the numbers a chess player would actually compare models by.
 */
export function chessRunLevelScores(
  result: RunResult
): readonly {
  name: string;
  metrics: Readonly<Record<string, { value: number }>>;
}[] {
  const games = result.sampleScores
    .filter((sample) => sample.score.value !== ScoreValue.Skipped)
    .flatMap((sample) => {
      const parsed = parseSchema(
        ChessGameRecordSchema,
        sample.metadata?.["game"]
      );
      return Either.isRight(parsed) ? [parsed.right] : [];
    });
  if (games.length === 0) {
    return [];
  }
  const average = (select: (game: (typeof games)[number]) => number): number =>
    games.reduce((sum, game) => sum + select(game), 0) / games.length;
  const values: Record<(typeof CHESS_RUN_METRICS)[number], number> = {
    points: average((game) => game.points),
    acpl: average((game) => game.acpl),
    illegalAttempts: average((game) => game.illegalAttempts),
    blunders: average((game) => game.blunders),
    gameLengthMoves: average((game) => game.gameLengthMoves),
  };
  return [
    {
      name: "chess",
      metrics: Object.fromEntries(
        CHESS_RUN_METRICS.map((key) => [key, { value: values[key] }])
      ),
    },
  ];
}

//#endregion

//#region Benchmark registration

function makeLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const configParsed = parseSchema(
    ChessBenchmarkConfigSchema,
    input.benchmarkConfig
  );
  if (Either.isLeft(configParsed)) {
    return layerFail(
      new Error(
        `chess received invalid benchmarkConfig: ${configParsed.left.message}`
      )
    );
  }
  const config = configParsed.right;

  const modelLayer =
    input.modelLayer ??
    makeOpenRouterModelLayer({
      apiKey: input.apiKey,
      model: config.model,
      ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
      sessionId: input.sessionId,
      ...(input.modelRetry !== undefined && { retry: input.modelRetry }),
    });

  const solverLayer = layerEffect(
    Solver,
    gen(function* () {
      const model = yield* Model;
      return chessSolver(model, {
        temperature: CHESS_TEMPERATURE,
        ...(config.endpointId !== undefined && {
          endpointId: config.endpointId,
        }),
        ...(config.maxTokens !== undefined && { maxTokens: config.maxTokens }),
        ...(config.reasoningEffort !== undefined && {
          reasoningEffort: config.reasoningEffort,
        }),
        ...(config.timeoutMs !== undefined && { timeoutMs: config.timeoutMs }),
      });
    })
  ).pipe(layerProvide(modelLayer));

  return layerMergeAll(
    makeChessDatasetLayer(input.datasetRetry),
    solverLayer,
    layerSucceed(Scorer, chessScorer)
  );
}

export const CHESS_BENCHMARK: Benchmark = {
  id: CHESS_META.id,
  makeDatasetLayer: makeChessDatasetLayer,
  makeLayer,
  temperature: CHESS_TEMPERATURE,
  defaultEpochs: CHESS_META.defaultEpochs,
  /*
   * A missing Stockfish binary or a UCI crash is a per-sample infrastructure
   * failure, not a model failure: degrade to Incorrect rather than aborting
   * the whole fan-out (mirrors terminal-bench / swe-atlas sandbox handling).
   */
  degradeSolverErrors: true,
  runLevelScores: chessRunLevelScores,
};

//#endregion
