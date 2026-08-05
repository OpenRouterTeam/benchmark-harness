import { Chess } from "chess.js";

/**
 * Chess game state machine — everything deterministic about a game, kept
 * separate from the solver so move extraction, result classification, and
 * adjudication rules are unit-testable without an engine or a model.
 *
 * Faithful port of the byo-benchmark chess bench (examples/benchmarks/chess):
 * same SAN extraction, same adjudication thresholds, same result taxonomy,
 * same scoring semantics (points 1/0.5/0, ACPL, blunders).
 */
import type { ValueOf } from "../../internal/guards";
import type { UciScore } from "./uci";

export const CHESS_TASKS = [
  "stockfish-full",
  "stockfish-full-black",
  "stockfish-full-validated",
  "stockfish-full-strict",
  "endgame-conversion",
] as const;
export type ChessTaskId = ValueOf<typeof CHESS_TASKS>;

export interface ChessTask {
  readonly id: ChessTaskId;
  /** Opponent search depth (deterministic weak opponent at low depth). */
  readonly engineDepth: number;
  /** Stockfish evaluation depth (position evals + best-move baseline). */
  readonly evalDepth: number;
  /** Optional starting FEN (endgames). */
  readonly fen?: string;
  /** Which side the model plays. */
  readonly modelColor: "w" | "b";
  /** Safety cap on model moves; hitting it adjudicates by final eval. */
  readonly maxPlies: number;
  /** Allow "check <move>" legality probes before committing a move. */
  readonly moveValidation?: boolean;
  /** Reply must be exactly the SAN move — no lenient extraction. */
  readonly strict?: boolean;
  /** endgame-conversion: only checkmate-win scores Correct. */
  readonly requireMate?: boolean;
}

/**
 * Full games vs a depth-2 opponent. maxPlies=120 is a cost/safety cap, not a
 * target — depth-2 games typically resolve well before it (mate, draw rule,
 * or the −900cp adjudication).
 */
const STANDARD = {
  engineDepth: 2,
  evalDepth: 10,
  modelColor: "w",
  maxPlies: 120,
} as const;

export const CHESS_TASK_DEFINITIONS: Readonly<Record<ChessTaskId, ChessTask>> =
  {
    "stockfish-full": { id: "stockfish-full", ...STANDARD },
    "stockfish-full-black": {
      id: "stockfish-full-black",
      ...STANDARD,
      modelColor: "b",
    },
    "stockfish-full-validated": {
      id: "stockfish-full-validated",
      ...STANDARD,
      moveValidation: true,
    },
    "stockfish-full-strict": {
      id: "stockfish-full-strict",
      ...STANDARD,
      strict: true,
    },
    "endgame-conversion": {
      id: "endgame-conversion",
      ...STANDARD,
      fen: "4k3/8/4K3/8/8/8/8/4Q3 w - - 0 1",
      maxPlies: 30,
      requireMate: true,
    },
  };

// ---------- move extraction ----------

const SAN_RE = /O-O-O|O-O|[KQRBN]?[a-h]?[1-8]?x?[a-h][1-8](?:=[QRBN])?[+#]?/g;

/** Try a move on a probe board; returns normalized SAN or undefined. */
export function tryMove(game: Chess, move: string): string | undefined {
  const probe = new Chess(game.fen());
  try {
    return probe.move(move).san;
  } catch {
    return undefined;
  }
}

/**
 * Strict mode: the reply must BE the canonical SAN for a legal move —
 * chess.js's parser is lenient (accepts e2e4, Ng1f3, sloppy check marks),
 * so legality alone would let non-SAN slip through unpenalized. The reply
 * must round-trip to itself modulo an omitted +/# suffix.
 */
export function tryStrictMove(game: Chess, reply: string): string | undefined {
  const san = tryMove(game, reply);
  if (san === undefined) {
    return undefined;
  }
  return reply === san || reply === san.replace(/[+#]$/, "") ? san : undefined;
}

/** Lenient: first token in the reply that is a legal move. */
export function extractMove(game: Chess, reply: string): string | undefined {
  for (const candidate of [reply.trim(), ...(reply.match(SAN_RE) ?? [])]) {
    const san = tryMove(game, candidate);
    if (san !== undefined) {
      return san;
    }
  }
  return undefined;
}

/** Convert side-to-move UCI score to the model's perspective. */
export function fromModelView(
  score: UciScore,
  fen: string,
  modelColor: "w" | "b"
): { cp: number; mateIn?: number } {
  const sideToMove = fen.split(" ")[1];
  const sign = sideToMove === modelColor ? 1 : -1;
  return {
    cp: sign * score.cp,
    ...(score.mateIn !== undefined && { mateIn: sign * score.mateIn }),
  };
}

// ---------- adjudication (engine-tournament style) ----------

export const MAX_CHECKS_PER_TURN = 3;
/** Position must stay at/below this (model view) to accumulate hopeless turns. */
export const ADJUDICATE_LOSS_CP = -900;
/** Consecutive hopeless model turns before adjudicating the loss. */
export const ADJUDICATE_LOSS_TURNS = 3;
/** |eval| needed to call a decisive result when the maxPlies cap is hit. */
export const ADJUDICATE_CAP_CP = 400;
/** A model move losing at least this many centipawns counts as a blunder. */
export const BLUNDER_CP = 200;
/** cpLoss is clamped here so one catastrophe can't dominate ACPL unboundedly. */
export const CP_LOSS_CLAMP = 1000;

export const GAME_RESULTS = [
  "checkmate-win",
  "checkmate-loss",
  "stalemate",
  "draw-repetition",
  "draw-insufficient",
  "draw-fifty-moves",
  "adjudicated-win",
  "adjudicated-loss",
  "draw-agreed-adjudication",
  "forfeit",
] as const;
export type GameResult = ValueOf<typeof GAME_RESULTS>;

export interface GameVerdict {
  readonly result: GameResult;
  readonly points: number;
}

/** Classify a finished (game-over) board from the model's perspective. */
export function boardResult(game: Chess, modelColor: "w" | "b"): GameVerdict {
  if (game.isCheckmate()) {
    const modelMated = game.turn() === modelColor;
    return {
      result: modelMated ? "checkmate-loss" : "checkmate-win",
      points: modelMated ? 0 : 1,
    };
  }
  if (game.isStalemate()) {
    return { result: "stalemate", points: 0.5 };
  }
  if (game.isThreefoldRepetition()) {
    return { result: "draw-repetition", points: 0.5 };
  }
  if (game.isInsufficientMaterial()) {
    return { result: "draw-insufficient", points: 0.5 };
  }
  // chess.js isDraw() covers the 50-move rule once the above are excluded.
  return { result: "draw-fifty-moves", points: 0.5 };
}

/** Adjudicate a live game that hit the maxPlies safety cap, by final eval. */
export function capVerdict(finalCp: number): GameVerdict {
  if (finalCp >= ADJUDICATE_CAP_CP) {
    return { result: "adjudicated-win", points: 1 };
  }
  if (finalCp <= -ADJUDICATE_CAP_CP) {
    return { result: "adjudicated-loss", points: 0 };
  }
  return { result: "draw-agreed-adjudication", points: 0.5 };
}

// ---------- per-game record ----------

/** One ply as recorded in the output: validation + evaluation at every stage. */
export interface PlyRecord {
  readonly ply: number;
  readonly by: "model" | "engine";
  readonly san: string;
  readonly fenAfter: string;
  /** Stockfish eval after this ply, centipawns from the MODEL's perspective. */
  readonly evalCp: number;
  /** Mate distance if the evaluator sees one (signed, model perspective). */
  readonly mateIn?: number;
  /** Model plies only: Stockfish's preferred move in this position (SAN). */
  readonly bestMove?: string;
  /** Model plies only: centipawns lost vs playing bestMove (0 = perfect). */
  readonly cpLoss?: number;
}

/**
 * Cost/usage for ONE model call (iteration) in the game — the atomic unit of
 * spend. `turn` is the game turn the call served; a turn can have several
 * iterations (illegal-move retry, "check <move>" probes). `generationId`
 * joins to billing, so reported cost is auditable against actual charges
 * per iteration rather than a self-reported total.
 */
export interface TurnCost {
  readonly iteration: number;
  readonly turn: number;
  readonly generationId?: string;
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly totalTokens: number;
  readonly reasoningTokens: number;
  readonly costUsd: number;
  readonly generationTimeMs: number;
}

export interface ChessGameRecord {
  readonly taskId: ChessTaskId;
  readonly modelMoves: readonly string[];
  readonly plies: readonly PlyRecord[];
  readonly illegalAttempts: number;
  readonly strictViolations: number;
  readonly checksUsed: number;
  readonly forfeited: boolean;
  readonly result: GameResult;
  /** Chess scoring: 1 = model won, 0.5 = draw, 0 = loss/forfeit. */
  readonly points: number;
  readonly gameLengthMoves: number;
  /** Average centipawn loss across the model's moves (engine-perfect = 0). */
  readonly acpl: number;
  readonly worstCpLoss: number;
  /** Model moves losing ≥ BLUNDER_CP (blunder count). */
  readonly blunders: number;
  readonly finalEvalCp: number;
  readonly finalFen: string;
  readonly pgn: string;
  /** Per-iteration cost ledger: one row per model call, billing-joinable. */
  readonly turnCosts: readonly TurnCost[];
  /** Sum of turnCosts[].costUsd — must equal the sample's reported usage. */
  readonly totalCostUsd: number;
}
