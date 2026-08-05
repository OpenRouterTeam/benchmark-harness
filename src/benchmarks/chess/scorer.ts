import { succeed } from "effect/Effect";

/**
 * Chess scorer — fully deterministic, no judge model.
 *
 * Primary verdict: game POINTS (win 1 / draw 0.5 / loss 0), thresholded at
 * a draw: a game the model didn't lose scores Correct. endgame-conversion is
 * stricter — only checkmate-win counts, because K+Q vs K is trivially won
 * and anything else means the model lost the position in its head.
 *
 * The full quality profile (ACPL, blunders, illegal attempts, worst move,
 * result taxonomy) rides on the explanation and run-level scores so the
 * numbers behind the verdict stay reviewable per sample.
 */
import type { Score } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import { ChessGameRecordSchema } from "./schema";

export const chessScorer: ScorerService = (state) => {
  const parsed = parseSchema(
    ChessGameRecordSchema,
    state.sample.metadata?.["game"]
  );
  if (Either.isLeft(parsed)) {
    const score: Score = {
      value: ScoreValue.Incorrect,
      answer: null,
      explanation: `no game record in sample metadata: ${parsed.left.message}`,
    };
    return succeed(score);
  }
  const game = parsed.right;

  const requireMate = game.taskId === "endgame-conversion";
  const won = requireMate
    ? game.result === "checkmate-win"
    : game.points >= 0.5;

  const quality =
    game.modelMoves.length > 0
      ? `ACPL ${game.acpl}cp, worst ${game.worstCpLoss}cp, ${game.blunders} blunder(s), ` +
        `${game.illegalAttempts} illegal attempt(s) over ${game.modelMoves.length} move(s)`
      : "no moves played";

  let explanation = `${game.result}: ${quality}`;
  if (requireMate) {
    explanation =
      game.result === "checkmate-win"
        ? `mated in ${game.modelMoves.length} — ${quality}`
        : `failed to convert a trivially won endgame (${game.result}) — ${quality}`;
  }

  const score: Score = {
    value: won ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: `${game.result} (${game.points} pts, ${game.gameLengthMoves} moves)`,
    explanation,
  };
  return succeed(score);
};
