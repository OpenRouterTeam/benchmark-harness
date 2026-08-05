import { describe, expect, it } from "bun:test";

import { Chess } from "chess.js";

import { ScoreValue } from "../../harness/core";
import { runHarnessPromise } from "../../internal/effect-logger";
import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import {
  boardResult,
  capVerdict,
  CHESS_TASK_DEFINITIONS,
  CHESS_TASKS,
  extractMove,
  fromModelView,
  tryMove,
} from "./game";
import { ChessGameRecordSchema, ChessTaskSchema } from "./schema";
import { chessScorer } from "./scorer";

describe("move extraction", () => {
  const start = new Chess();

  it("accepts a bare SAN move and normalizes it", () => {
    expect(tryMove(start, "Nf3")).toBe("Nf3");
    expect(tryMove(start, "e4")).toBe("e4");
  });

  it("rejects illegal moves without mutating the board", () => {
    const fenBefore = start.fen();
    expect(tryMove(start, "Ke2")).toBeUndefined();
    expect(start.fen()).toBe(fenBefore);
  });

  it("extracts the first legal move from chatty replies", () => {
    expect(extractMove(start, "I will play e4, controlling the center.")).toBe(
      "e4"
    );
    expect(extractMove(start, "My move: Nf3!")).toBe("Nf3");
  });

  it("extracts castling and promotion notation", () => {
    const castled = new Chess(
      "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4"
    );
    expect(extractMove(castled, "O-O")).toBe("O-O");
    const promoting = new Chess("8/4P3/8/8/8/8/2k5/K7 w - - 0 1");
    expect(extractMove(promoting, "e8=Q")).toBe("e8=Q");
  });

  it("returns undefined when no legal move is present", () => {
    expect(extractMove(start, "I resign, you play too well.")).toBeUndefined();
  });
});

describe("eval perspective", () => {
  it("keeps sign when the side to move is the model", () => {
    const start = new Chess();
    expect(fromModelView({ cp: 35 }, start.fen(), "w").cp).toBe(35);
  });

  it("flips sign when the opponent is to move", () => {
    const start = new Chess();
    expect(fromModelView({ cp: 35 }, start.fen(), "b").cp).toBe(-35);
    expect(fromModelView({ cp: -120, mateIn: -3 }, start.fen(), "b")).toEqual({
      cp: 120,
      mateIn: 3,
    });
  });
});

describe("result classification", () => {
  it("classifies checkmate for and against the model", () => {
    /* Fool's mate: black mates white. */
    const mated = new Chess();
    for (const move of ["f3", "e5", "g4", "Qh4#"]) {
      mated.move(move);
    }
    expect(boardResult(mated, "w")).toEqual({
      result: "checkmate-loss",
      points: 0,
    });
    expect(boardResult(mated, "b")).toEqual({
      result: "checkmate-win",
      points: 1,
    });
  });

  it("classifies stalemate as a half point", () => {
    const stalemate = new Chess("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1");
    expect(boardResult(stalemate, "w")).toEqual({
      result: "stalemate",
      points: 0.5,
    });
  });

  it("adjudicates the maxPlies cap by final eval", () => {
    expect(capVerdict(450)).toEqual({ result: "adjudicated-win", points: 1 });
    expect(capVerdict(-450)).toEqual({ result: "adjudicated-loss", points: 0 });
    expect(capVerdict(120)).toEqual({
      result: "draw-agreed-adjudication",
      points: 0.5,
    });
  });
});

describe("task definitions", () => {
  it("every task parses through the metadata schema", () => {
    for (const taskId of CHESS_TASKS) {
      assertRight(parseSchema(ChessTaskSchema, CHESS_TASK_DEFINITIONS[taskId]));
    }
  });

  it("the endgame task starts from the K+Q vs K FEN and requires mate", () => {
    const endgame = CHESS_TASK_DEFINITIONS["endgame-conversion"];
    expect(endgame.fen).toBe("4k3/8/4K3/8/8/8/8/4Q3 w - - 0 1");
    expect(endgame.requireMate).toBe(true);
    expect(endgame.maxPlies).toBe(30);
  });
});

function gameRecord(
  overrides: Record<string, unknown> = {}
): Record<string, unknown> {
  return {
    taskId: "stockfish-full",
    modelMoves: ["e4", "Nf3"],
    plies: [],
    illegalAttempts: 0,
    strictViolations: 0,
    checksUsed: 0,
    forfeited: false,
    result: "checkmate-win",
    points: 1,
    gameLengthMoves: 20,
    acpl: 45,
    worstCpLoss: 130,
    blunders: 0,
    finalEvalCp: 900,
    finalFen: "8/8/8/8/8/8/8/K1k5 w - - 0 1",
    pgn: "1. e4",
    turnCosts: [
      {
        iteration: 0,
        turn: 0,
        generationId: "gen-abc",
        inputTokens: 120,
        outputTokens: 4,
        totalTokens: 124,
        reasoningTokens: 0,
        costUsd: 0.0004,
        generationTimeMs: 800,
      },
      {
        iteration: 1,
        turn: 1,
        inputTokens: 160,
        outputTokens: 4,
        totalTokens: 164,
        reasoningTokens: 0,
        costUsd: 0.0005,
        generationTimeMs: 750,
      },
    ],
    totalCostUsd: 0.0009,
    ...overrides,
  };
}

describe("chessScorer", () => {
  const score = (metadata: Record<string, unknown>) =>
    runHarnessPromise(
      chessScorer(
        {
          sample: { id: "chess-x", input: "", target: { text: "" }, metadata },
          messages: [],
          completed: true,
        },
        { text: "" }
      )
    );

  it("scores a win Correct with the quality profile in the explanation", async () => {
    const result = await score({ game: gameRecord() });
    expect(result.value).toBe(ScoreValue.Correct);
    expect(result.explanation).toContain("ACPL 45cp");
  });

  it("scores a draw Correct (points >= 0.5) on standard tasks", async () => {
    const result = await score({
      game: gameRecord({ result: "stalemate", points: 0.5 }),
    });
    expect(result.value).toBe(ScoreValue.Correct);
  });

  it("scores a loss and a forfeit Incorrect", async () => {
    const loss = await score({
      game: gameRecord({ result: "checkmate-loss", points: 0 }),
    });
    expect(loss.value).toBe(ScoreValue.Incorrect);
    const forfeit = await score({
      game: gameRecord({ result: "forfeit", points: 0, forfeited: true }),
    });
    expect(forfeit.value).toBe(ScoreValue.Incorrect);
  });

  it("endgame-conversion requires checkmate — a draw scores Incorrect", async () => {
    const drawn = await score({
      game: gameRecord({
        taskId: "endgame-conversion",
        result: "stalemate",
        points: 0.5,
      }),
    });
    expect(drawn.value).toBe(ScoreValue.Incorrect);
    expect(drawn.explanation).toContain("failed to convert");
    const mated = await score({
      game: gameRecord({
        taskId: "endgame-conversion",
        result: "checkmate-win",
        points: 1,
      }),
    });
    expect(mated.value).toBe(ScoreValue.Correct);
  });

  it("scores Incorrect when the game record is missing or malformed", async () => {
    const missing = await score({});
    expect(missing.value).toBe(ScoreValue.Incorrect);
    expect(missing.explanation).toContain("no game record");
  });

  it("the record round-trips through its schema", () => {
    assertRight(parseSchema(ChessGameRecordSchema, gameRecord()));
  });

  it("turn costs are the atomic spend ledger: totalCostUsd equals their sum", () => {
    const parsed = parseSchema(ChessGameRecordSchema, gameRecord());
    assertRight(parsed);
    const summed = parsed.right.turnCosts.reduce(
      (acc, cost) => acc + cost.costUsd,
      0
    );
    expect(parsed.right.totalCostUsd).toBeCloseTo(summed, 10);
    /* Each iteration is billing-joinable when the API returned an id. */
    expect(parsed.right.turnCosts[0]?.generationId).toBe("gen-abc");
  });
});
