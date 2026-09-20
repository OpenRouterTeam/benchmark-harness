import type { Color, Move, PieceSymbol, Square } from "chess.js";
import { Chess, SQUARES } from "chess.js";

import { z } from "../../internal/zod";

export const PIECE_VALUES: Readonly<Record<PieceSymbol, number>> = {
  p: 1,
  n: 3,
  b: 3,
  r: 5,
  q: 9,
  k: 0,
};

export const PIECE_NAMES: Readonly<Record<PieceSymbol, string>> = {
  p: "pawn",
  n: "knight",
  b: "bishop",
  r: "rook",
  q: "queen",
  k: "king",
};

const CENTER_SQUARES: readonly Square[] = ["d4", "e4", "d5", "e5"];

export const CandidateFeaturesSchema = z.object({
  movedPiece: z.string(),
  capturedPiece: z.string().nullable(),
  capturedValue: z.number(),
  materialNet: z.number(),
  movedPieceHangs: z.boolean(),
  ownHangingAfter: z.number(),
  givesCheck: z.boolean(),
  isMate: z.boolean(),
  ownKingAttackedNeighbors: z.number(),
  opponentKingEscapeSquares: z.number(),
  opponentHangingAfter: z.number(),
  highestAttackedOpponentValue: z.number(),
  opponentMobility: z.number(),
  centerControl: z.number(),
});

export type CandidateFeatures = z.infer<typeof CandidateFeaturesSchema>;

function opposite(color: Color): Color {
  return color === "w" ? "b" : "w";
}

function squaresOf(chess: Chess, color: Color): readonly Square[] {
  return SQUARES.filter((square) => chess.get(square)?.color === color);
}

function minAttackerValue(
  chess: Chess,
  square: Square,
  by: Color
): number | undefined {
  const values = chess
    .attackers(square, by)
    .map((from) => chess.get(from))
    .flatMap((piece) => (piece === undefined ? [] : [PIECE_VALUES[piece.type]]))
    .map((value) => (value === 0 ? Number.POSITIVE_INFINITY : value));
  return values.length === 0 ? undefined : Math.min(...values);
}

function isHanging(chess: Chess, square: Square, owner: Color): boolean {
  const piece = chess.get(square);
  if (piece === undefined || piece.type === "k") {
    return false;
  }
  const attacker = minAttackerValue(chess, square, opposite(owner));
  if (attacker === undefined) {
    return false;
  }
  const defended = chess.attackers(square, owner).length > 0;
  return !defended || attacker < PIECE_VALUES[piece.type];
}

function expectedLossOnDestination(
  chess: Chess,
  square: Square,
  mover: Color,
  pieceValue: number
): number {
  const attacker = minAttackerValue(chess, square, opposite(mover));
  if (attacker === undefined) {
    return 0;
  }
  const defended = chess.attackers(square, mover).length > 0;
  if (!defended) {
    return pieceValue;
  }
  return attacker < pieceValue ? pieceValue - attacker : 0;
}

function kingNeighbors(king: Square): readonly Square[] {
  const file = king.charCodeAt(0);
  const rank = king.charCodeAt(1);
  const neighbors: Square[] = [];
  for (const df of [-1, 0, 1]) {
    for (const dr of [-1, 0, 1]) {
      if (df === 0 && dr === 0) {
        continue;
      }
      const candidate = String.fromCharCode(file + df, rank + dr);
      const match = SQUARES.find((square) => square === candidate);
      if (match !== undefined) {
        neighbors.push(match);
      }
    }
  }
  return neighbors;
}

function kingSquare(chess: Chess, color: Color): Square | undefined {
  return chess.findPiece({ type: "k", color })[0];
}

export function computeCandidateFeatures(
  before: Chess,
  move: Move
): CandidateFeatures {
  const mover = move.color;
  const opponent = opposite(mover);
  const after = new Chess(before.fen());
  after.move(move.san);
  const landedPiece: PieceSymbol = move.promotion ?? move.piece;
  const landedValue = PIECE_VALUES[landedPiece];
  const capturedValue =
    move.captured === undefined ? 0 : PIECE_VALUES[move.captured];
  const isMate = after.isCheckmate();
  const expectedLoss = expectedLossOnDestination(
    after,
    move.to,
    mover,
    landedValue
  );
  const ownKing = kingSquare(after, mover);
  const opponentKing = kingSquare(after, opponent);
  const opponentPieces = squaresOf(after, opponent);
  const attackedOpponentValues = opponentPieces
    .filter((square) => after.attackers(square, mover).length > 0)
    .map((square) => {
      const piece = after.get(square);
      return piece === undefined ? 0 : PIECE_VALUES[piece.type];
    });
  return {
    movedPiece: PIECE_NAMES[move.piece],
    capturedPiece:
      move.captured === undefined ? null : PIECE_NAMES[move.captured],
    capturedValue,
    materialNet: capturedValue - expectedLoss,
    movedPieceHangs: expectedLoss > 0,
    ownHangingAfter: squaresOf(after, mover).filter((square) =>
      isHanging(after, square, mover)
    ).length,
    givesCheck: after.inCheck(),
    isMate,
    ownKingAttackedNeighbors:
      ownKing === undefined
        ? 0
        : kingNeighbors(ownKing).filter((square) =>
            after.isAttacked(square, opponent)
          ).length,
    opponentKingEscapeSquares:
      opponentKing === undefined
        ? 0
        : after.moves({ square: opponentKing, verbose: true }).length,
    opponentHangingAfter: opponentPieces.filter((square) =>
      isHanging(after, square, opponent)
    ).length,
    highestAttackedOpponentValue: Math.max(0, ...attackedOpponentValues),
    opponentMobility: after.moves().length,
    centerControl: CENTER_SQUARES.filter((square) =>
      after.isAttacked(square, mover)
    ).length,
  };
}
