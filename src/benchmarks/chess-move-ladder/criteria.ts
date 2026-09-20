import type { CandidateFeatures } from "./features";
import type { CriterionId, HeuristicsVariant } from "./schema";
import { CRITERION_IDS } from "./schema";

interface Criterion {
  readonly rule: Readonly<Record<HeuristicsVariant, string>>;
  readonly describe: (features: CandidateFeatures) => readonly string[];
}

function signed(value: number): string {
  return value > 0 ? `+${value}` : `${value}`;
}

function forcingDescription(f: CandidateFeatures): string {
  if (f.isMate) {
    return "delivers checkmate";
  }
  return f.givesCheck ? "gives check" : "does not give check";
}

export const CRITERIA: Readonly<Record<CriterionId, Criterion>> = {
  material: {
    rule: {
      correct:
        "Material: prefer moves that win material (pawn=1, knight=3, bishop=3, rook=5, queen=9) after accounting for the opponent's best immediate recapture. A capture that is immediately recaptured by a cheaper piece loses material.",
      inverted:
        "Material: prefer moves that give up material (pawn=1, knight=3, bishop=3, rook=5, queen=9); a capture that can be recaptured is stronger than one that cannot.",
    },
    describe: (f) => [
      f.capturedPiece === null
        ? "captures nothing"
        : `captures a ${f.capturedPiece} (${f.capturedValue})`,
      `net material after the opponent's best immediate recapture: ${signed(f.materialNet)}`,
    ],
  },
  hanging: {
    rule: {
      correct:
        "Hanging pieces: do not leave your own pieces where they can be taken for free or by a cheaper attacker. Avoid moves that put the moved piece on an inadequately defended square.",
      inverted:
        "Hanging pieces: leaving your own pieces on squares where they can be taken for free or by a cheaper attacker is a sign of a strong move.",
    },
    describe: (f) => [
      f.movedPieceHangs
        ? "the moved piece can be taken profitably on its new square"
        : "the moved piece is safe on its new square",
      `own pieces left hanging after the move: ${f.ownHangingAfter}`,
    ],
  },
  forcing: {
    rule: {
      correct:
        "Forcing moves: checkmate ends the game and beats everything else. Checks and other forcing moves that restrict the opponent's replies are stronger than quiet moves, provided they are safe.",
      inverted:
        "Forcing moves: avoid checks and checkmates; quiet moves that leave the opponent many replies are stronger.",
    },
    describe: (f) => [forcingDescription(f)],
  },
  king_safety: {
    rule: {
      correct:
        "King safety: prefer moves that attack the opponent's king and reduce its escape squares, and avoid moves that expose your own king to attack.",
      inverted:
        "King safety: prefer moves that expose your own king and leave the opponent's king with many escape squares.",
    },
    describe: (f) => [
      `opponent king escape squares after the move: ${f.opponentKingEscapeSquares}`,
      `squares next to your own king attacked by the opponent after the move: ${f.ownKingAttackedNeighbors}`,
    ],
  },
  threats: {
    rule: {
      correct:
        "Threats: prefer moves that create new threats, such as attacking undefended enemy pieces or high-value pieces, because they force the opponent to respond.",
      inverted:
        "Threats: avoid moves that attack undefended or high-value enemy pieces; threats are a weakness.",
    },
    describe: (f) => [
      `opponent pieces left hanging after the move: ${f.opponentHangingAfter}`,
      `highest-value opponent piece attacked after the move: ${f.highestAttackedOpponentValue}`,
    ],
  },
  mobility: {
    rule: {
      correct:
        "Mobility and center: prefer moves that limit the opponent's legal replies and control the central squares d4, e4, d5, e5.",
      inverted:
        "Mobility and center: prefer moves that maximize the opponent's legal replies and avoid controlling the central squares d4, e4, d5, e5.",
    },
    describe: (f) => [
      `opponent legal replies after the move: ${f.opponentMobility}`,
      `central squares you attack after the move: ${f.centerControl}`,
    ],
  },
};

export function activeCriteria(
  ablate: CriterionId | undefined
): readonly CriterionId[] {
  return CRITERION_IDS.filter((id) => id !== ablate);
}

export function heuristicRules(
  variant: HeuristicsVariant,
  ablate: CriterionId | undefined
): readonly string[] {
  return activeCriteria(ablate).map((id) => CRITERIA[id].rule[variant]);
}

export function describeFeatures(
  features: CandidateFeatures,
  ablate: CriterionId | undefined
): readonly string[] {
  return activeCriteria(ablate).flatMap((id) =>
    CRITERIA[id].describe(features)
  );
}
