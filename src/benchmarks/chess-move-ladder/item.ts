import type { Move } from "chess.js";
import { Chess } from "chess.js";

import { Either } from "../../internal/either";
import { parseSchema, z } from "../../internal/zod";
import { seededPermutation } from "../scorers/mcq/shuffle";
import type { CandidateFeatures } from "./features";
import { CandidateFeaturesSchema, computeCandidateFeatures } from "./features";

export const PuzzleRecordSchema = z.object({
  PuzzleId: z.string().min(1),
  FEN: z.string().min(1),
  Moves: z.string().min(1),
  Rating: z.number().int(),
  Themes: z.array(z.string()).nullable(),
});

export type PuzzleRecord = z.infer<typeof PuzzleRecordSchema>;

export const LETTERS = "ABCDEFGH";

export const ChessCandidateSchema = z.object({
  letter: z.string().length(1),
  san: z.string().min(1),
  uci: z.string().min(4),
  features: CandidateFeaturesSchema,
});

export type ChessCandidate = z.infer<typeof ChessCandidateSchema>;

export const ChessItemSchema = z.object({
  puzzleId: z.string(),
  fen: z.string(),
  ascii: z.string(),
  sideToMove: z.enum(["white", "black"]),
  rating: z.number().int(),
  themes: z.array(z.string()),
  candidates: z.array(ChessCandidateSchema).min(1),
  solutionLetter: z.string().length(1),
  solutionSan: z.string(),
});

export type ChessItem = z.infer<typeof ChessItemSchema>;

export function hashString(text: string): number {
  let hash = 2166136261;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 16777619) >>> 0;
  }
  return hash >>> 0;
}

function temptingness(features: CandidateFeatures): number {
  return (
    features.capturedValue +
    (features.givesCheck ? 2 : 0) +
    (features.isMate ? 100 : 0)
  );
}

function loadPosition(puzzleId: string, fen: string): Chess {
  const loaded = Either.try(() => new Chess(fen));
  if (Either.isLeft(loaded)) {
    throw new TypeError(
      `chess_move_ladder puzzle ${puzzleId} has an invalid FEN: ${fen}`
    );
  }
  return loaded.right;
}

interface ScoredMove {
  readonly move: Move;
  readonly features: CandidateFeatures;
}

export function selectDistractors(
  scored: readonly ScoredMove[],
  count: number,
  seed: number
): readonly ScoredMove[] {
  if (count <= 0 || scored.length === 0) {
    return [];
  }
  const order = seededPermutation(scored.length, seed);
  const shuffled = order.map((index) => scored[index]!);
  const byTempting = [...shuffled].sort(
    (a, b) => temptingness(b.features) - temptingness(a.features)
  );
  const temptingCount = Math.min(Math.ceil(count / 2), byTempting.length);
  const tempting = byTempting.slice(0, temptingCount);
  const remaining = shuffled.filter((entry) => !tempting.includes(entry));
  return [...tempting, ...remaining.slice(0, count - tempting.length)];
}

export function puzzleRecordToItem(
  record: Readonly<Record<string, unknown>>,
  candidateCount: number
): ChessItem {
  const parsed = parseSchema(PuzzleRecordSchema, record);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `chess_move_ladder record failed validation: ${parsed.left.message}`
    );
  }
  const puzzle = parsed.right;
  const [setupUci, solutionUci] = puzzle.Moves.split(" ");
  if (setupUci === undefined || solutionUci === undefined) {
    throw new TypeError(
      `chess_move_ladder puzzle ${puzzle.PuzzleId} needs at least two moves`
    );
  }
  const chess = loadPosition(puzzle.PuzzleId, puzzle.FEN);
  const setup = Either.try(() => chess.move(setupUci));
  if (Either.isLeft(setup)) {
    throw new TypeError(
      `chess_move_ladder puzzle ${puzzle.PuzzleId} has an illegal setup move ${setupUci}`
    );
  }
  const legal = chess.moves({ verbose: true });
  const solution = legal.find((move) => move.lan === solutionUci);
  if (solution === undefined) {
    throw new TypeError(
      `chess_move_ladder puzzle ${puzzle.PuzzleId} has an illegal solution move ${solutionUci}`
    );
  }
  const scored = legal
    .filter((move) => move.lan !== solutionUci)
    .map((move) => ({ move, features: computeCandidateFeatures(chess, move) }))
    .filter((entry) => !entry.features.isMate);
  const seed = hashString(puzzle.PuzzleId);
  const distractors = selectDistractors(scored, candidateCount - 1, seed);
  const unordered: readonly ScoredMove[] = [
    { move: solution, features: computeCandidateFeatures(chess, solution) },
    ...distractors,
  ];
  const order = seededPermutation(unordered.length, seed + 1);
  const candidates = order.map((sourceIndex, position) => {
    const entry = unordered[sourceIndex]!;
    return {
      letter: LETTERS[position]!,
      san: entry.move.san,
      uci: entry.move.lan,
      features: entry.features,
    };
  });
  const solutionLetter = candidates.find(
    (candidate) => candidate.uci === solutionUci
  )!.letter;
  return {
    puzzleId: puzzle.PuzzleId,
    fen: chess.fen(),
    ascii: chess.ascii(),
    sideToMove: chess.turn() === "w" ? "white" : "black",
    rating: puzzle.Rating,
    themes: puzzle.Themes ?? [],
    candidates,
    solutionLetter,
    solutionSan: solution.san,
  };
}

export function parseChessItem(
  metadata: Readonly<Record<string, unknown>> | undefined
): Either.Either<ChessItem, string> {
  const parsed = parseSchema(ChessItemSchema, metadata?.["item"]);
  return Either.isLeft(parsed)
    ? Either.left(parsed.left.message)
    : Either.right(parsed.right);
}
