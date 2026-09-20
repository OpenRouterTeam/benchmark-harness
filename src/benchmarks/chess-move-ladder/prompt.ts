import type { SystemOneRequest } from "../../providers/systemone-model";
import { describeFeatures, heuristicRules } from "./criteria";
import type { ChessCandidate, ChessItem } from "./item";
import type { ChessCondition, CriteriaLevel } from "./schema";

const META_THEMES: ReadonlySet<string> = new Set([
  "oneMove",
  "short",
  "long",
  "veryLong",
  "opening",
  "middlegame",
  "endgame",
  "master",
  "masterVsMaster",
  "superGM",
  "crushing",
  "advantage",
  "equality",
]);

export function motifThemes(item: ChessItem): readonly string[] {
  return item.themes.filter((theme) => !META_THEMES.has(theme));
}

const LEVEL_RANK: Readonly<Record<CriteriaLevel, number>> = {
  board: 0,
  instruction: 1,
  heuristics: 2,
  features: 3,
  motif: 4,
};

export function levelAtLeast(
  level: CriteriaLevel,
  floor: CriteriaLevel
): boolean {
  return LEVEL_RANK[level] >= LEVEL_RANK[floor];
}

export function renderBoard(
  item: ChessItem,
  condition: ChessCondition
): readonly string[] {
  switch (condition.boardFormat) {
    case "fen": {
      return [`FEN: ${item.fen}`];
    }
    case "ascii": {
      return [
        "Board (uppercase = white, lowercase = black):",
        item.ascii.trimEnd(),
      ];
    }
    case "both": {
      return [
        `FEN: ${item.fen}`,
        "Board (uppercase = white, lowercase = black):",
        item.ascii.trimEnd(),
      ];
    }
  }
}

export function instructionFor(item: ChessItem, level: CriteriaLevel): string {
  const side = item.sideToMove;
  switch (level) {
    case "board": {
      return `It is ${side} to move. Select one of the candidate moves.`;
    }
    case "instruction":
    case "heuristics":
    case "features":
    case "motif": {
      return `It is ${side} to move. Select the candidate move that is objectively strongest for ${side}: the move a strong chess engine would choose.`;
    }
  }
}

export function candidateDescription(
  candidate: ChessCandidate,
  condition: ChessCondition
): string {
  if (!levelAtLeast(condition.criteriaLevel, "features")) {
    return candidate.san;
  }
  const facts = describeFeatures(candidate.features, condition.ablateCriterion);
  return `${candidate.san}: ${facts.join("; ")}`;
}

export function heuristicsBlock(condition: ChessCondition): readonly string[] {
  if (!levelAtLeast(condition.criteriaLevel, "heuristics")) {
    return [];
  }
  return heuristicRules(condition.heuristicsVariant, condition.ablateCriterion);
}

function motifBlock(
  item: ChessItem,
  condition: ChessCondition
): string | undefined {
  if (condition.criteriaLevel !== "motif") {
    return undefined;
  }
  const motifs = motifThemes(item);
  return motifs.length === 0
    ? "A tactical opportunity exists for the side to move."
    : `The position contains a tactic for the side to move of type: ${motifs.join(", ")}.`;
}

export function renderChatPrompt(
  item: ChessItem,
  condition: ChessCondition
): string {
  const letters = item.candidates.map((candidate) => candidate.letter);
  const rules = heuristicsBlock(condition);
  const motif = motifBlock(item, condition);
  const featuresNote = levelAtLeast(condition.criteriaLevel, "features")
    ? [
        "Each candidate is annotated with facts computed by a chess library from the position after the move.",
      ]
    : [];
  return [
    instructionFor(item, condition.criteriaLevel),
    "",
    ...renderBoard(item, condition),
    "",
    ...(rules.length > 0
      ? ["Heuristics for choosing a move, in priority order:", ...rules, ""]
      : []),
    ...(motif !== undefined ? [motif, ""] : []),
    ...featuresNote,
    "Candidate moves:",
    ...item.candidates.map(
      (candidate) =>
        `${candidate.letter}) ${candidateDescription(candidate, condition)}`
    ),
    "",
    `The last line of your response should be of the following format: 'Answer: $LETTER' (without quotes) where LETTER is one of ${letters.join(", ")}.`,
  ].join("\n");
}

export const SYSTEMONE_QUESTION_ID = "best_move";

export function renderSystemOneRequest(
  item: ChessItem,
  condition: ChessCondition
): SystemOneRequest {
  const rules = heuristicsBlock(condition);
  const motif = motifBlock(item, condition);
  const state: Record<string, unknown> = {
    side_to_move: item.sideToMove,
    ...(condition.boardFormat !== "ascii" && { fen: item.fen }),
    ...(condition.boardFormat !== "fen" && { board: item.ascii.trimEnd() }),
    ...(rules.length > 0 && { heuristics: rules }),
    ...(motif !== undefined && { tactical_motif: motif }),
    candidate_moves: item.candidates.map((candidate) => ({
      id: candidate.letter,
      move: candidate.san,
    })),
  };
  const criteria = Object.fromEntries(
    item.candidates.map((candidate) => [
      candidate.letter,
      candidateDescription(candidate, condition),
    ])
  );
  return {
    state,
    questions: {
      [SYSTEMONE_QUESTION_ID]: {
        type: "choice",
        instructions: instructionFor(item, condition.criteriaLevel),
        criteria,
      },
    },
  };
}
