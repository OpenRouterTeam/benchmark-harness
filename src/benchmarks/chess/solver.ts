import { Chess } from "chess.js";
import type { Effect } from "effect/Effect";
import {
  either as effectEither,
  ensuring,
  fail as effectFail,
  gen,
  sync,
  tryPromise,
} from "effect/Effect";

import type { ReasoningEffort } from "../../harness/constants";
/**
 * Chess solver: plays a full game against a real UCI engine (Stockfish),
 * one model turn at a time. The model never sees a board or FEN after the
 * first message — only the move history — so every legal move is evidence it
 * still knows where the pieces are.
 *
 * Games run END TO END: checkmate, draw (50-move / repetition / stalemate /
 * insufficient material), forfeit (two illegal replies on one turn), or
 * adjudication. Engine-tournament adjudication keeps cost bounded: a position
 * that stays hopeless (≤ −900cp) for 3 consecutive model turns is an
 * adjudicated loss; hitting the maxPlies cap adjudicates by final eval.
 *
 * Every ply is validated with chess.js and evaluated by Stockfish at fixed
 * depth (independent of the opponent's search, so evals are comparable across
 * cases and runs). Scoring is fully deterministic — no judge model anywhere.
 *
 * Concurrency: each sample spawns its own opponent + evaluator engine pair
 * (Threads=1 each) and quits them in ensuring(). Games share NOTHING — no
 * engine handles, no boards, no mutable module state — so any harness
 * concurrency level is safe; the practical bound is 2 processes per live game.
 */
import type { ChatMessage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { unknownErrorToString } from "../../internal/errors";
import { parseSchema } from "../../internal/zod";
import type { GameVerdict, PlyRecord, TurnCost } from "./game";
import {
  ADJUDICATE_LOSS_CP,
  ADJUDICATE_LOSS_TURNS,
  BLUNDER_CP,
  boardResult,
  capVerdict,
  CP_LOSS_CLAMP,
  extractMove,
  fromModelView,
  MAX_CHECKS_PER_TURN,
  tryMove,
  tryStrictMove,
} from "./game";
import { ChessTaskSchema } from "./schema";
import { MATE_CP, UciEngine } from "./uci";

export interface ChessSolverOpts {
  readonly temperature: number;
  readonly endpointId?: string;
  readonly maxTokens?: number;
  readonly reasoningEffort?: ReasoningEffort;
  readonly timeoutMs?: number;
}

/**
 * Engine interactions fail as typed SolverError, never as a defect: with
 * degradeSolverErrors the run engine degrades that one game to Incorrect
 * (missing binary, engine hang) instead of tearing down the whole fan-out.
 */
function engineCall<A>(run: () => Promise<A>): Effect<A, SolverError> {
  return tryPromise({
    try: run,
    catch: (error) =>
      new SolverError({
        message: unknownErrorToString(error),
      }),
  });
}

export function chessSolver(
  model: ModelService,
  opts: ChessSolverOpts
): SolverService {
  return (taskState) => {
    /* Declared OUTSIDE the effect and cleaned in an outer ensuring(): an
     * interrupt landing between an engine's spawn and the inner cleanup
     * installation must still quit whatever started. */
    const engines: UciEngine[] = [];
    return gen(function* () {
      const taskMeta = parseSchema(
        ChessTaskSchema,
        taskState.sample.metadata?.["task"]
      );
      if (Either.isLeft(taskMeta)) {
        return yield* new SolverError({
          message: `chess sample carries no valid task metadata: ${taskMeta.left.message}`,
        });
      }
      const task = taskMeta.right;
      const game = new Chess(task.fen);

      /* Fail closed before any model spend: no Stockfish, no chess run. */
      const opponent = yield* engineCall(() => UciEngine.start());
      engines.push(opponent);
      /* A failed evaluator start propagates as SolverError; the outer
       * ensuring() quits the already-started opponent. */
      const evaluatorResult = yield* effectEither(
        engineCall(() => UciEngine.start())
      );
      if (Either.isLeft(evaluatorResult)) {
        return yield* evaluatorResult.left;
      }
      const evaluator = evaluatorResult.right;
      engines.push(evaluator);

      return yield* gen(function* () {
        opponent.newGame();
        evaluator.newGame();

        const plies: PlyRecord[] = [];
        const modelMoves: string[] = [];
        let illegalAttempts = 0;
        let strictViolations = 0;
        let checksUsed = 0;
        let forfeited = false;
        const accUsage = {
          inputTokens: 0,
          outputTokens: 0,
          totalTokens: 0,
          reasoningTokens: 0,
          totalCost: 0,
        };
        let totalGenerationTimeMs = 0;
        /* Atomic cost ledger: one row per model call (iteration), each with
         * its generation id so cost joins to billing per iteration — never
         * only a self-reported aggregate (the Terminal-Bench gap). */
        const turnCosts: TurnCost[] = [];
        let currentTurn = 0;

        /* Eval from the model's view; terminal positions short-circuit. */
        const evalNow = engineCall(
          async (): Promise<{ cp: number; mateIn?: number }> => {
            if (game.isCheckmate()) {
              const modelMated = game.turn() === task.modelColor;
              return { cp: modelMated ? -MATE_CP : MATE_CP, mateIn: 0 };
            }
            if (game.isDraw()) {
              return { cp: 0 };
            }
            const searched = await evaluator.search(game.fen(), task.evalDepth);
            return fromModelView(searched.score, game.fen(), task.modelColor);
          }
        );

        function record(input: {
          readonly by: "model" | "engine";
          readonly san: string;
          readonly extra?: Partial<PlyRecord>;
          /* Pass the eval when the caller already searched this position —
           * re-searching an identical fixed-depth position doubles engine
           * time per model ply for a bit-identical result. */
          readonly evaluated?: { cp: number; mateIn?: number };
        }): Effect<void, SolverError> {
          const { by, san, extra = {} } = input;
          return gen(function* () {
            const resolved = input.evaluated ?? (yield* evalNow);
            plies.push({
              ply: plies.length,
              by,
              san,
              fenAfter: game.fen(),
              evalCp: resolved.cp,
              ...(resolved.mateIn !== undefined && { mateIn: resolved.mateIn }),
              ...extra,
            });
          });
        }

        const colorName = task.modelColor === "w" ? "White" : "Black";
        const strictRule =
          task.strict === true
            ? "Reply with EXACTLY your move in standard algebraic notation (SAN) and nothing else — no punctuation, no commentary, no move numbers. Example replies: Nf3 | exd5 | O-O | e8=Q+"
            : 'Reply with ONLY your next move in standard algebraic notation (SAN), e.g. "Nf3", "exd5", "O-O". No commentary, no move numbers.';
        const validationRule =
          task.moveValidation === true
            ? `Before committing, you may reply "check <move>" (e.g. "check Nf3") to verify a move is legal without playing it. You get ${MAX_CHECKS_PER_TURN} checks per turn. A bare move commits it.`
            : "";
        const rules = [
          `You are playing a chess game as ${colorName}.`,
          strictRule,
          validationRule,
        ]
          .filter((line) => line !== "")
          .join("\n");
        const messages: ChatMessage[] = [
          { role: MessageRole.System, content: rules },
        ];

        // Opening user message carries the full starting context; moves only after.
        let situation =
          task.fen === undefined ? "" : `Position (FEN): ${task.fen}\n`;

        // If it's not the model's turn, the engine opens.
        if (game.turn() !== task.modelColor && !game.isGameOver()) {
          const opening = yield* engineCall(() =>
            opponent.search(game.fen(), task.engineDepth)
          );
          const san = game.move(opening.bestmove).san;
          situation += `Your opponent played: ${san}\n`;
          yield* record({ by: "engine", san });
        }
        messages.push({
          role: MessageRole.User,
          content: `${situation}Your move.`,
        });

        const generateTurn = gen(function* () {
          const output = yield* model.generate(messages, {
            temperature: opts.temperature,
            ...(opts.endpointId !== undefined && {
              endpointId: opts.endpointId,
            }),
            ...(opts.maxTokens !== undefined && { maxTokens: opts.maxTokens }),
            ...(opts.reasoningEffort !== undefined && {
              reasoningEffort: opts.reasoningEffort,
            }),
            ...(opts.timeoutMs !== undefined && { timeoutMs: opts.timeoutMs }),
          });
          totalGenerationTimeMs += output.generationTimeMs ?? 0;
          accUsage.inputTokens += output.usage?.inputTokens ?? 0;
          accUsage.outputTokens += output.usage?.outputTokens ?? 0;
          accUsage.totalTokens += output.usage?.totalTokens ?? 0;
          accUsage.reasoningTokens += output.usage?.reasoningTokens ?? 0;
          accUsage.totalCost += output.usage?.totalCost ?? 0;
          turnCosts.push({
            iteration: turnCosts.length,
            turn: currentTurn,
            ...(output.generationId !== undefined && {
              generationId: output.generationId,
            }),
            inputTokens: output.usage?.inputTokens ?? 0,
            outputTokens: output.usage?.outputTokens ?? 0,
            totalTokens: output.usage?.totalTokens ?? 0,
            reasoningTokens: output.usage?.reasoningTokens ?? 0,
            costUsd: output.usage?.totalCost ?? 0,
            generationTimeMs: output.generationTimeMs ?? 0,
          });
          return output.completion;
        });

        let hopelessTurns = 0;
        let terminated: GameVerdict | undefined;

        for (let turn = 0; turn < task.maxPlies && !game.isGameOver(); turn++) {
          currentTurn = turn;
          // Stockfish's best move BEFORE the model moves — the cpLoss baseline.
          const before = yield* engineCall(() =>
            evaluator.search(game.fen(), task.evalDepth)
          );
          const bestSan = tryMove(game, before.bestmove);
          const beforeCp = fromModelView(
            before.score,
            game.fen(),
            task.modelColor
          ).cp;

          // Adjudicate hopeless positions (engine-tournament style).
          hopelessTurns =
            beforeCp <= ADJUDICATE_LOSS_CP ? hopelessTurns + 1 : 0;
          if (hopelessTurns >= ADJUDICATE_LOSS_TURNS) {
            terminated = { result: "adjudicated-loss", points: 0 };
            break;
          }

          let san: string | undefined;
          let checksThisTurn = 0;

          // One retry on an illegal/unparseable reply; a second failure forfeits.
          for (let attempt = 0; attempt < 2 && san === undefined;) {
            const reply = yield* generateTurn;
            messages.push({ role: MessageRole.Assistant, content: reply });
            const trimmed = reply.trim();

            /* moveValidation probes don't commit and don't count as attempts
             * — but only up to the per-turn limit. Past it, a probe consumes
             * an attempt like any non-move reply, so a model that only ever
             * probes forfeits after two more replies instead of looping the
             * turn (and its spend) forever. */
            const probe =
              task.moveValidation === true
                ? trimmed.match(/^check[:\s]+(\S+)$/i)
                : null;
            if (
              probe?.[1] !== undefined &&
              checksThisTurn < MAX_CHECKS_PER_TURN
            ) {
              checksUsed++;
              checksThisTurn++;
              const legal = tryMove(game, probe[1]);
              messages.push({
                role: MessageRole.User,
                content:
                  legal === undefined
                    ? `"${probe[1]}" is NOT legal here.`
                    : `"${probe[1]}" is legal. Reply with a move to play it.`,
              });
              continue;
            }
            if (probe?.[1] !== undefined) {
              // Probe past the limit: burns an attempt.
              attempt++;
              illegalAttempts++;
              if (attempt < 2) {
                messages.push({
                  role: MessageRole.User,
                  content: "Check limit reached. Reply with your move.",
                });
              }
              continue;
            }

            san =
              task.strict === true
                ? tryStrictMove(game, trimmed)
                : extractMove(game, trimmed);
            if (
              task.strict === true &&
              san === undefined &&
              extractMove(game, trimmed) !== undefined
            ) {
              strictViolations++;
            }
            if (san === undefined) {
              attempt++;
              illegalAttempts++;
              if (attempt < 2) {
                messages.push({
                  role: MessageRole.User,
                  content: `"${trimmed}" is not a legal move${
                    task.strict === true
                      ? " (reply with the exact SAN move only)"
                      : ""
                  }. Legal moves: ${game.moves().join(", ")}. Your move.`,
                });
              }
            }
          }
          if (san === undefined) {
            forfeited = true;
            terminated = { result: "forfeit", points: 0 };
            break;
          }

          game.move(san);
          modelMoves.push(san);
          // cpLoss: eval swing vs the evaluator's preferred move (model's view).
          const afterEvaluated = yield* evalNow;
          const cpLoss = Math.min(
            CP_LOSS_CLAMP,
            Math.max(0, beforeCp - afterEvaluated.cp)
          );
          yield* record({
            by: "model",
            san,
            extra: {
              ...(bestSan !== undefined && { bestMove: bestSan }),
              cpLoss,
            },
            evaluated: afterEvaluated,
          });
          if (game.isGameOver()) {
            break;
          }

          const engineMove = yield* engineCall(() =>
            opponent.search(game.fen(), task.engineDepth)
          );
          /* Validate before applying: a malformed/empty bestmove (engine
           * killed mid-search, protocol hiccup) must degrade this game via
           * SolverError, not throw an untyped chess.js exception. */
          const engineSan = tryMove(game, engineMove.bestmove);
          if (engineSan === undefined) {
            return yield* effectFail(
              new SolverError({
                message: `engine returned an unplayable bestmove "${engineMove.bestmove}" at ${game.fen()}`,
              })
            );
          }
          game.move(engineSan);
          yield* record({ by: "engine", san: engineSan });
          messages.push({
            role: MessageRole.User,
            content: `Your opponent played: ${engineSan}. Your move.`,
          });
        }

        let verdict: GameVerdict;
        if (terminated !== undefined) {
          verdict = terminated;
        } else if (game.isGameOver()) {
          verdict = boardResult(game, task.modelColor);
        } else {
          // maxPlies safety cap hit with a live game: adjudicate by final eval.
          const finalEvaluated = yield* evalNow;
          verdict = capVerdict(finalEvaluated.cp);
        }

        const modelPlies = plies.filter((ply) => ply.by === "model");
        const acpl =
          modelPlies.length > 0
            ? Math.round(
                modelPlies.reduce((acc, ply) => acc + (ply.cpLoss ?? 0), 0) /
                  modelPlies.length
              )
            : 0;
        const worstCpLoss = modelPlies.reduce(
          (acc, ply) => Math.max(acc, ply.cpLoss ?? 0),
          0
        );
        const blunders = modelPlies.filter(
          (ply) => (ply.cpLoss ?? 0) >= BLUNDER_CP
        ).length;

        const gameRecord = {
          taskId: task.id,
          modelMoves,
          plies,
          illegalAttempts,
          strictViolations,
          checksUsed,
          forfeited,
          result: verdict.result,
          points: verdict.points,
          gameLengthMoves: Math.ceil(game.history().length / 2),
          acpl,
          worstCpLoss,
          blunders,
          finalEvalCp: plies.at(-1)?.evalCp ?? 0,
          finalFen: game.fen(),
          pgn: game.pgn(),
          turnCosts,
          totalCostUsd: accUsage.totalCost,
        };

        const completion = JSON.stringify({
          result: verdict.result,
          points: verdict.points,
          moves: modelMoves.length,
        });

        return {
          ...taskState,
          sample: {
            ...taskState.sample,
            metadata: { ...taskState.sample.metadata, game: gameRecord },
          },
          messages,
          output: {
            completion,
            message: { role: MessageRole.Assistant, content: completion },
            usage: accUsage,
            generationTimeMs: totalGenerationTimeMs,
          },
          completed: true,
        };
      });
    }).pipe(
      ensuring(
        sync(() => {
          for (const engine of engines) {
            engine.quit();
          }
        })
      )
    );
  };
}
