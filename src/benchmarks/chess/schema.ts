import { z } from "../../internal/zod";
/**
 * Zod schemas for chess task metadata and the per-game record. The task
 * schema round-trips through sample metadata (dataset → solver), and the
 * game-record schema is what the scorer reads back — parse, don't cast.
 */
import type { ChessGameRecord, ChessTask, PlyRecord, TurnCost } from "./game";
import { CHESS_TASKS, GAME_RESULTS } from "./game";

export const ChessTaskSchema = z.object({
  id: z.enum(CHESS_TASKS),
  engineDepth: z.number().int().min(1),
  evalDepth: z.number().int().min(1),
  fen: z.string().optional(),
  modelColor: z.enum(["w", "b"]),
  maxPlies: z.number().int().min(1),
  moveValidation: z.boolean().optional(),
  strict: z.boolean().optional(),
  requireMate: z.boolean().optional(),
}) satisfies z.ZodType<ChessTask>;
export type ChessTaskParsed = z.infer<typeof ChessTaskSchema>;

export const ChessPlyRecordSchema = z.object({
  ply: z.number().int(),
  by: z.enum(["model", "engine"]),
  san: z.string(),
  fenAfter: z.string(),
  evalCp: z.number(),
  mateIn: z.number().optional(),
  bestMove: z.string().optional(),
  cpLoss: z.number().optional(),
}) satisfies z.ZodType<PlyRecord>;

export const ChessTurnCostSchema = z.object({
  iteration: z.number().int(),
  turn: z.number().int(),
  generationId: z.string().optional(),
  inputTokens: z.number(),
  outputTokens: z.number(),
  totalTokens: z.number(),
  reasoningTokens: z.number(),
  costUsd: z.number(),
  generationTimeMs: z.number(),
}) satisfies z.ZodType<TurnCost>;

export const ChessGameRecordSchema = z.object({
  taskId: z.enum(CHESS_TASKS),
  modelMoves: z.array(z.string()),
  plies: z.array(ChessPlyRecordSchema),
  illegalAttempts: z.number().int(),
  strictViolations: z.number().int(),
  checksUsed: z.number().int(),
  forfeited: z.boolean(),
  result: z.enum(GAME_RESULTS),
  points: z.number(),
  gameLengthMoves: z.number().int(),
  acpl: z.number(),
  worstCpLoss: z.number(),
  blunders: z.number().int(),
  finalEvalCp: z.number(),
  finalFen: z.string(),
  pgn: z.string(),
  turnCosts: z.array(ChessTurnCostSchema),
  totalCostUsd: z.number(),
}) satisfies z.ZodType<ChessGameRecord>;
export type ChessGameRecordParsed = z.infer<typeof ChessGameRecordSchema>;
