import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { chessRecordToSample } from "./benchmark";
import {
  MATE_IN_ONE_RECORD,
  PUZZLE_RECORD,
  TWO_MATES_RECORD,
} from "./fixtures";
import { parseChessItem, puzzleRecordToItem } from "./item";

describe("puzzleRecordToItem", () => {
  it("applies the setup move and targets the first solver move", () => {
    const item = puzzleRecordToItem(PUZZLE_RECORD, 4);
    expect(item.fen).toBe(
      "r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2b1/PqP3PP/7K w - - 0 25"
    );
    expect(item.sideToMove).toBe("white");
    expect(item.solutionSan).toBe("Rxe7");
    expect(item.candidates).toHaveLength(4);
    const solution = item.candidates.find(
      (candidate) => candidate.letter === item.solutionLetter
    );
    expect(solution?.uci).toBe("e6e7");
    expect(item.candidates.map((candidate) => candidate.letter)).toEqual([
      "A",
      "B",
      "C",
      "D",
    ]);
  });
  it("is deterministic for the same record", () => {
    expect(puzzleRecordToItem(PUZZLE_RECORD, 4)).toEqual(
      puzzleRecordToItem(PUZZLE_RECORD, 4)
    );
  });
  it("computes features that mark checkmate", () => {
    const item = puzzleRecordToItem(MATE_IN_ONE_RECORD, 3);
    const solution = item.candidates.find(
      (candidate) => candidate.letter === item.solutionLetter
    );
    expect(solution?.san).toBe("Qxf7#");
    expect(solution?.features.isMate).toBe(true);
    expect(solution?.features.givesCheck).toBe(true);
    expect(solution?.features.capturedPiece).toBe("pawn");
  });
  it("excludes alternative mating moves from the distractors", () => {
    const item = puzzleRecordToItem(TWO_MATES_RECORD, 8);
    const mates = item.candidates.filter(
      (candidate) => candidate.features.isMate
    );
    expect(mates.map((candidate) => candidate.san)).toEqual(["Qd8#"]);
  });
  it("never duplicates the solution among the distractors", () => {
    const item = puzzleRecordToItem(PUZZLE_RECORD, 6);
    const ucis = item.candidates.map((candidate) => candidate.uci);
    expect(new Set(ucis).size).toBe(ucis.length);
  });
  it("rejects records missing required fields", () => {
    expect(() =>
      puzzleRecordToItem({ ...PUZZLE_RECORD, FEN: undefined }, 4)
    ).toThrow(TypeError);
  });
  it("rejects puzzles with an illegal setup move", () => {
    expect(() =>
      puzzleRecordToItem({ ...PUZZLE_RECORD, Moves: "a1a2 e6e7" }, 4)
    ).toThrow(/illegal setup move/u);
  });
  it("rejects puzzles with a single move", () => {
    expect(() =>
      puzzleRecordToItem({ ...PUZZLE_RECORD, Moves: "f2g3" }, 4)
    ).toThrow(/at least two moves/u);
  });
});

describe("chessRecordToSample", () => {
  it("round-trips the item through sample metadata", () => {
    const sample = chessRecordToSample(PUZZLE_RECORD, 4);
    expect(sample.id).toBe("chess_move_ladder-00008");
    const parsed = parseChessItem(sample.metadata);
    assertRight(parsed);
    expect(parsed.right.solutionLetter).toBe(sample.target.text);
    expect(parsed.right.puzzleId).toBe("00008");
  });
  it("rejects metadata that is not a chess item", () => {
    const parsed = parseChessItem({ puzzleId: 1 });
    expect(parsed._tag).toBe("Left");
  });
});
