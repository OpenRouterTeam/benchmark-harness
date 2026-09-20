import { describe, expect, it } from "bun:test";

import { CRITERIA } from "./criteria";
import { PUZZLE_RECORD } from "./fixtures";
import { puzzleRecordToItem } from "./item";
import {
  motifThemes,
  renderChatPrompt,
  renderSystemOneRequest,
  SYSTEMONE_QUESTION_ID,
} from "./prompt";
import type { ChessCondition } from "./schema";

const ITEM = puzzleRecordToItem(PUZZLE_RECORD, 4);

function condition(overrides: Partial<ChessCondition>): ChessCondition {
  return {
    criteriaLevel: "instruction",
    heuristicsVariant: "correct",
    boardFormat: "both",
    ...overrides,
  };
}

const MATERIAL_RULE = CRITERIA.material.rule.correct;
const INVERTED_MATERIAL_RULE = CRITERIA.material.rule.inverted;

describe("renderChatPrompt", () => {
  it("keeps the board and candidates identical across criteria levels", () => {
    const board = renderChatPrompt(ITEM, condition({ criteriaLevel: "board" }));
    const features = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "features" })
    );
    expect(board).toContain(`FEN: ${ITEM.fen}`);
    expect(features).toContain(`FEN: ${ITEM.fen}`);
    for (const candidate of ITEM.candidates) {
      expect(board).toContain(`${candidate.letter}) ${candidate.san}`);
      expect(features).toContain(`${candidate.letter}) ${candidate.san}:`);
    }
  });
  it("omits the strength goal at the board level", () => {
    const prompt = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "board" })
    );
    expect(prompt).not.toContain("objectively strongest");
    expect(prompt).not.toContain("Heuristics");
    expect(prompt).not.toContain("net material");
  });
  it("states the goal but no heuristics at the instruction level", () => {
    const prompt = renderChatPrompt(ITEM, condition({}));
    expect(prompt).toContain("objectively strongest");
    expect(prompt).not.toContain(MATERIAL_RULE);
    expect(prompt).not.toContain("net material");
  });
  it("adds rules without computed features at the heuristics level", () => {
    const prompt = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "heuristics" })
    );
    expect(prompt).toContain(MATERIAL_RULE);
    expect(prompt).not.toContain("net material");
  });
  it("adds computed features at the features level", () => {
    const prompt = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "features" })
    );
    expect(prompt).toContain(MATERIAL_RULE);
    expect(prompt).toContain("net material after the opponent's best");
  });
  it("adds the puzzle motif only at the motif level", () => {
    const features = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "features" })
    );
    const motif = renderChatPrompt(ITEM, condition({ criteriaLevel: "motif" }));
    expect(features).not.toContain("hangingPiece");
    expect(motif).toContain("hangingPiece");
    expect(motif).not.toContain("middlegame");
  });
  it("swaps in inverted rules for false-criterion injection", () => {
    const prompt = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "heuristics", heuristicsVariant: "inverted" })
    );
    expect(prompt).toContain(INVERTED_MATERIAL_RULE);
    expect(prompt).not.toContain(MATERIAL_RULE);
  });
  it("drops both the rule and the feature of an ablated criterion", () => {
    const prompt = renderChatPrompt(
      ITEM,
      condition({ criteriaLevel: "features", ablateCriterion: "material" })
    );
    expect(prompt).not.toContain(MATERIAL_RULE);
    expect(prompt).not.toContain("net material");
    expect(prompt).toContain(CRITERIA.forcing.rule.correct);
  });
  it("respects the board format", () => {
    const fen = renderChatPrompt(ITEM, condition({ boardFormat: "fen" }));
    const ascii = renderChatPrompt(ITEM, condition({ boardFormat: "ascii" }));
    expect(fen).toContain("FEN:");
    expect(fen).not.toContain("+---");
    expect(ascii).not.toContain("FEN:");
    expect(ascii).toContain("+---");
  });
  it("ends with the answer-format instruction listing the letters", () => {
    const prompt = renderChatPrompt(ITEM, condition({}));
    expect(prompt.trimEnd().endsWith("A, B, C, D.")).toBe(true);
  });
});

describe("renderSystemOneRequest", () => {
  it("emits one choice question keyed by candidate letter", () => {
    const request = renderSystemOneRequest(ITEM, condition({}));
    const question = request.questions[SYSTEMONE_QUESTION_ID];
    expect(question?.type).toBe("choice");
    expect(Object.keys(question?.criteria ?? {})).toEqual(["A", "B", "C", "D"]);
  });
  it("puts heuristics in state only from the heuristics level", () => {
    const bare = renderSystemOneRequest(ITEM, condition({}));
    const rich = renderSystemOneRequest(
      ITEM,
      condition({ criteriaLevel: "heuristics" })
    );
    expect(bare.state).not.toHaveProperty("heuristics");
    expect(rich.state).toHaveProperty("heuristics");
  });
  it("annotates choice criteria with features at the features level", () => {
    const request = renderSystemOneRequest(
      ITEM,
      condition({ criteriaLevel: "features" })
    );
    const question = request.questions[SYSTEMONE_QUESTION_ID];
    expect(question?.criteria["A"]).toContain("net material");
  });
});

describe("motifThemes", () => {
  it("strips length and phase tags", () => {
    expect(motifThemes(ITEM)).toEqual(["hangingPiece"]);
  });
});
